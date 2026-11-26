//! WebRTC signalling WebSocket connection loop, authentication and message dispatch.

use super::{
    LIST_APPS, Limits, PROBE_CHECK, Peer, REPLACED, SignedIn, app_list, artwork, capabilities,
    gateway_status, pairing::Pairing,
};
use crate::{
    clients::{Close, Registered, Registry},
    discovery,
    session::{self, Command, Commands, Request},
};
use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use butterpollo_session::webrtc::{
    auth::Challenge,
    capabilities::{EncoderProbe, FrameRateOffer},
    protocol::{
        ClientMessage, ErrorCode, ProtocolError, ServerMessage, SessionState, parse_client_message,
    },
};
use butterpollo_stream::{
    network::Throttle, state::Shared, stream::Media, webrtc_bridge::ConsoleSignIn,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::{Sender, channel};

/// What a connection does after a message.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Flow {
    Continue,
    Close,
}

pub(super) struct Authenticated {
    pub(super) client_id: String,
    pub(super) name: String,
}

pub(super) struct Connection {
    pub(super) h: Shared,
    pub(super) clients: Arc<Registry>,
    pub(super) id: u64,
    pub(super) peer: Peer,
    pub(super) challenge: Challenge,
    pub(super) client: Option<Authenticated>,
    pub(super) pairing: Option<Pairing>,
    /// What the last `capabilities` was built from: the encoder probe and the frame rate offer.
    pub(super) offered: Option<(EncoderProbe, FrameRateOffer)>,
    pub(super) limits: Limits,
    /// When a connection that has not authenticated is closed.
    pub(super) unauthenticated_until: tokio::time::Instant,
    /// The stream pipeline, and where the session actor's replies go.
    streaming: Option<Streaming>,
    /// The session actor, started with the first session request.
    pub(super) session: Option<Commands>,
    /// This connection's page has said why this session's peer connection failed
    /// ([`client_line_is_loud`]).
    pub(super) reported_failure: bool,
}

/// Whether a `client-log` line goes in a default log, with how many were kept out of it since the
/// last that went in; the others are for debugging. A page's report that its peer connection failed
/// has what the host can't see, the reason and the browser. A connection gets one such line per
/// session, and a paired page reconnecting in a loop gets no more than `throttle` lets through.
pub(super) fn client_line_is_loud(
    line: &str,
    reported: &mut bool,
    throttle: &mut Throttle,
) -> Option<u32> {
    if !line.starts_with("connection failed:") || std::mem::replace(reported, true) {
        return None;
    }
    throttle.allow()
}

/// The host-wide limit on the failures pages report ([`client_line_is_loud`]).
static CLIENT_FAILURES: std::sync::Mutex<Throttle> = std::sync::Mutex::new(Throttle::new());

struct Streaming {
    media: Arc<Media>,
    replies: Sender<ServerMessage>,
}

impl Connection {
    pub(super) fn new(
        h: Shared,
        clients: Arc<Registry>,
        limits: Limits,
        peer: Peer,
        id: u64,
    ) -> Self {
        Self {
            challenge: clients.challenge(),
            h,
            clients,
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn connection(
    h: Shared,
    clients: Arc<Registry>,
    limits: Limits,
    media: Option<Arc<Media>>,
    peer: Peer,
    console: Option<(ConsoleSignIn, Duration)>,
    server_check: Option<Duration>,
    mut stopped: Option<tokio::sync::watch::Receiver<bool>>,
    mut socket: WebSocket,
) {
    let ip = peer.address.ip().to_canonical();
    let registered = match &console {
        Some((sign_in, _)) => Some(clients.open_authenticated(ip, &sign_in.client_id)),
        None => clients.open(ip),
    };
    let Some(Registered { id, mut closed }) = registered else {
        tracing::debug!(peer = %peer.address, "WebRTC connection refused: this peer has all the unauthenticated connections the host allows");
        let _ = send_frame(&mut socket, Message::Close(None)).await;
        return;
    };
    // Dropping the state frees the connection's slots and pairing request,
    // however the task ends: a normal exit, a panic or an abort.
    let mut state = Connection::new(h.clone(), clients.clone(), limits, peer, id);
    let (replies, mut replied) = channel(REPLY_CAP);
    state.streaming = media.map(|media| Streaming { media, replies });
    tracing::debug!(peer = %peer.address, id, "WebRTC connection opened");
    let mut changed = clients.subscribe();
    let mut tick = tokio::time::interval(PROBE_CHECK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut out = Vec::new();
    // A console connection is signed in already; its sign-in and the WebRTC server setting are
    // checked every few seconds. A play connection checks only the WebRTC server setting.
    let mut signed_in = None;
    let mut play_check = server_check.map(|every| {
        let mut check = tokio::time::interval(every);
        check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        check
    });
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
            drop(state);
            close_socket(socket, close.take()).await;
            return;
        }
        let idle_until = state
            .client
            .is_none()
            .then_some(state.unauthenticated_until);
        // Biased: a host-side close, console revocation, a pairing result and the
        // idle limit are answered before a busy peer's next message.
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
            () = server_stopped(&mut stopped) => {
                tracing::debug!(peer = %peer.address, id, "WebRTC connection closed: server stopped");
                drop(state);
                let _ = send(&mut socket, &mut out).await;
                close_socket(
                    socket,
                    Some(CloseFrame {
                        code: 1001,
                        reason: "The host stopped the WebRTC server".into(),
                    }),
                )
                .await;
                return;
            }
            reason = console_revoked(&h, &mut signed_in) => {
                tracing::info!(peer = %peer.address, id, reason, "WebRTC console connection closed");
                drop(state);
                let _ = send(&mut socket, &mut out).await;
                close_socket(
                    socket,
                    Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: reason.into(),
                    }),
                )
                .await;
                return;
            }
            reason = webrtc_server_off(&h, &mut play_check) => {
                tracing::info!(peer = %peer.address, id, reason, "WebRTC play connection closed");
                drop(state);
                let _ = send(&mut socket, &mut out).await;
                close_socket(
                    socket,
                    Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: reason.into(),
                    }),
                )
                .await;
                return;
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

async fn webrtc_server_off(h: &Shared, check: &mut Option<tokio::time::Interval>) -> &'static str {
    let Some(every) = check else {
        return std::future::pending().await;
    };
    loop {
        every.tick().await;
        if !h.config.read().unwrap().webrtc_server() {
            return "the WebRTC server is off";
        }
    }
}

async fn server_stopped(stopped: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(rx) = stopped.as_mut() else {
        return std::future::pending().await;
    };
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
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

const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_GRACE: Duration = Duration::from_millis(200);
const REPLY_CAP: usize = 128;

pub(super) async fn send_within<F, E>(limit: Duration, fut: F) -> bool
where
    F: std::future::Future<Output = Result<(), E>>,
{
    matches!(tokio::time::timeout(limit, fut).await, Ok(Ok(())))
}

async fn send_frame(socket: &mut WebSocket, message: Message) -> bool {
    send_within(SEND_TIMEOUT, socket.send(message)).await
}

async fn close_socket(mut socket: WebSocket, frame: Option<CloseFrame>) {
    if !send_frame(&mut socket, Message::Close(frame)).await {
        return;
    }
    // Wait briefly while draining in-flight peer frames for close/EOF so dropping the socket on
    // Windows does not trigger a TCP RST while the Close frame is still unread.
    let _ = tokio::time::timeout(CLOSE_GRACE, async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
}

/// The Wake-on-LAN address `auth-required` offers a connection `opened` so: `lookup`'s for a TV
/// app or program on `webrtc_port`, none for anything else.
pub(super) fn wake_address(
    opened: super::Opened,
    lookup: impl FnOnce() -> Option<String>,
) -> Option<String> {
    (opened == super::Opened::PlainApp).then(lookup).flatten()
}

/// Send what is queued. False when the socket is gone or stalled.
async fn send(socket: &mut WebSocket, out: &mut Vec<ServerMessage>) -> bool {
    for message in out.drain(..) {
        if !send_frame(socket, Message::Text(message.to_json().into())).await {
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
    /// The first message to a connection that has not signed in. Only a TV app or program on
    /// `webrtc_port` gets the Wake-on-LAN address in it, for its reachability probe; a web page
    /// gets it in `gateway-status` once it has signed in ([`super::Opened`]).
    pub(super) fn auth_required(&self) -> ServerMessage {
        let local = self.peer.local.ip();
        let mac = wake_address(self.peer.opened, || discovery::mac_address(local));
        ServerMessage::auth_required(self.challenge.nonce().unwrap_or_default(), mac)
    }

    pub(super) fn error(code: ErrorCode, request_type: &str, message: &str) -> ServerMessage {
        ProtocolError::new(code, message).into_message(request_type)
    }

    /// The device this connection authenticated as, if it is still paired and enabled.
    fn recheck(&mut self, out: &mut Vec<ServerMessage>) -> Flow {
        let Some(client) = &self.client else {
            return Flow::Continue;
        };
        match self.clients.verify(&self.h, &client.client_id) {
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

    pub(super) async fn handle(&mut self, text: &str, out: &mut Vec<ServerMessage>) -> Flow {
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
                    if !session.send(Command::Stop) {
                        out.push(Self::error(
                            ErrorCode::InternalError,
                            kind,
                            "The session manager is busy or stopped",
                        ));
                    }
                }
                None => out.push(ServerMessage::session_status(SessionState::Idle, None)),
            },
            ClientMessage::StopHostSession => self.command(kind, Command::StopHost, out),
            ClientMessage::Answer { session_id, sdp } => {
                // An answer with no session behind it is late signalling.
                match &self.session {
                    Some(session) => {
                        if !session.send(Command::Answer { session_id, sdp }) {
                            tracing::debug!(
                                id = self.id,
                                kind,
                                session_id,
                                "WebRTC answer dropped: session inbox full or closed"
                            );
                            out.push(Self::error(
                                ErrorCode::InternalError,
                                kind,
                                "The session manager is busy or stopped",
                            ));
                        }
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
            self.clients
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
        let secret = self.clients.secret(&self.h, &client_id);
        if !self.challenge.verify(secret.as_deref(), proof) {
            tracing::info!(peer = %self.peer.address, "WebRTC authentication failed");
            out.push(Self::error(
                ErrorCode::AuthenticationFailed,
                "authenticate",
                "Authentication failed",
            ));
            return;
        }
        let client = match self.clients.verify(&self.h, &client_id) {
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
        if let Err(code) = self.clients.authenticate(self.id, &client_id) {
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

    /// The messages that follow `authenticated` and `paired`.
    pub(super) fn startup(&mut self, out: &mut Vec<ServerMessage>) {
        out.push(gateway_status(&self.h, self.peer.local.ip()));
        let (capabilities, offered) = capabilities(&self.h);
        self.offered = Some(offered);
        out.push(capabilities);
    }

    /// `capabilities` again when the encoder probe finished or changed, or `webrtc_frame_rates`
    /// changed the rates on offer.
    pub(super) fn refresh_capabilities(&mut self, out: &mut Vec<ServerMessage>) {
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
            self.clients.close(self.id);
        }));
    }
}
