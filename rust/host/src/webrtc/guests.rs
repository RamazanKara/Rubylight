//! Guests of the console's browser stream (docs/browser-guests.md): the invite links the owner
//! hands out, the guests who joined with one, and each guest's socket and media transport.
//!
//! The owner signs in to the console and streams on its Stream page. While that stream runs, a
//! guest with a link opens `/guest/`, whose page signals over `/guest/signal`: the token first,
//! then an offer for the owner's very stream. The guest's transport follows the owner's
//! ([`Followers`]): the same encoded frames and Opus packets, packetized, encrypted and paced by
//! its own media thread, so a slow guest drops frames alone and never delays the owner. Its
//! controllers and keys go through the owner's input worker ([`GuestHandle`]), which checks them
//! against the invite's permission. The owner's stream ending ends every guest's.
//!
//! Nothing here reaches the console: the guest routes sit outside its sign-in, and a guest's
//! socket speaks only this module's few messages.

use super::{
    input::{GuestHandle, InputHandle},
    media::{self, Followers, Hooks, MediaHandle, Pacing, TransportEvent, TransportOptions},
    sdp::{self, OfferSpec, VideoCodec},
};
use crate::state::{Session, Shared};
use anyhow::{Context, Result};
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
};
use butterpollo_core::{
    audio::OpusLayout,
    webrtc::{
        guests::{Invite, InviteError, Invites, Lookups, Permission, guest_name},
        protocol::{ControlMessage, classify_control},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{Arc, Mutex, Weak, atomic::AtomicBool},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// The guest page's WebSocket subprotocol.
pub const PROTOCOL: &str = "rubylight.guest.v1";
/// Guests at once. Guest `n` (0-based) gets UDP port `webrtc_media_port + 1 + n`.
pub const MAX_GUESTS: u16 = 4;
/// Guest sockets open at once, joined or not, in all and from one address (an IPv6 /64).
const MAX_SOCKETS: usize = 16;
const MAX_SOCKETS_PER_PEER: usize = 4;
/// How long a page has to send its token, and to answer the offer.
const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an answered guest may take to connect its media.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a guest waits for its media port, which the guest before it may still hold.
const BIND_WAIT: Duration = Duration::from_secs(3);
/// Keyframes the guests may ask the owner's encoder for: one this often, between them all. The
/// encoder makes one picture for everyone, so a guest on a poor link must not keep the owner's in
/// keyframes; a guest that asks too soon is asked for again by its transport.
pub const KEYFRAME_INTERVAL: Duration = Duration::from_secs(2);
const MAX_MESSAGE: usize = 64 * 1024;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The key sockets are counted under: the address, an IPv6 address by its /64.
fn peer_key(peer: IpAddr) -> IpAddr {
    match peer.to_canonical() {
        IpAddr::V6(address) => {
            std::net::Ipv6Addr::from(u128::from(address) & (u128::MAX << 64)).into()
        }
        v4 => v4,
    }
}

/// The owner's running console stream, which guests join.
#[derive(Clone)]
pub struct Owner {
    pub spec: OfferSpec,
    /// The multiopus layout the owner's browser answered with; None for stereo.
    pub audio_layout: Option<OpusLayout>,
    pub followers: Arc<Followers>,
    pub input: InputHandle,
    pub session: Weak<Session>,
}

/// Why the host ended a guest's stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leave {
    Kicked,
    Revoked,
    OwnerEnded,
}

impl Leave {
    fn said(self) -> (&'static str, &'static str) {
        match self {
            Self::Kicked => ("kicked", "The host removed you from the stream."),
            Self::Revoked => ("revoked", "The host withdrew this invite link."),
            Self::OwnerEnded => ("ended", "The host's stream ended."),
        }
    }
}

/// Why a guest could not join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// Too many wrong tokens from this address lately.
    RateLimited,
    /// No live invite has this token: wrong, expired or revoked.
    InvalidLink,
    NotStreaming,
    Full,
}

impl JoinError {
    fn said(self) -> (&'static str, String) {
        match self {
            Self::RateLimited => (
                "rate-limited",
                "Too many wrong links came from this network; wait a minute and try again.".into(),
            ),
            Self::InvalidLink => (
                "invalid-link",
                "This invite link does not work: it expired, the host withdrew it, or it was copied incompletely.".into(),
            ),
            Self::NotStreaming => (
                "not-streaming",
                "The host is not streaming right now. This page joins when the stream starts.".into(),
            ),
            Self::Full => (
                "full",
                format!("{MAX_GUESTS} guests are watching already; try again when one leaves."),
            ),
        }
    }
}

/// A guest let in: who it is, the stream it joins, and where the host tells it to leave.
pub struct Admission {
    pub id: u64,
    /// Its place among the guests, which picks its media port.
    pub slot: u16,
    pub name: String,
    pub permission: Permission,
    pub owner: Owner,
    pub leave: UnboundedReceiver<Leave>,
}

struct Joined {
    slot: u16,
    name: String,
    permission: Permission,
    invite: String,
    since: u64,
    leave: UnboundedSender<Leave>,
}

#[derive(Default)]
struct Inner {
    invites: Invites,
    lookups: Lookups,
    /// The owner's stream and the number it was registered under.
    owner: Option<(u64, Owner)>,
    generation: u64,
    joined: BTreeMap<u64, Joined>,
    next_guest: u64,
    keyframe_at: Option<Instant>,
    sockets: HashMap<IpAddr, usize>,
}

/// The invites and guests of the host. Invites live in memory only: a restart revokes them all.
#[derive(Default)]
pub struct Guests {
    inner: Mutex<Inner>,
}

#[derive(Serialize)]
pub struct InviteInfo {
    pub id: String,
    pub permission: Permission,
    pub created: u64,
    pub expires: u64,
}

#[derive(Serialize)]
pub struct GuestInfo {
    pub id: u64,
    pub name: String,
    pub permission: Permission,
    pub invite: String,
    pub since: u64,
}

/// What the console's guest panel shows. Never a token: the host does not have them.
#[derive(Serialize)]
pub struct Status {
    pub streaming: bool,
    pub max_guests: u16,
    pub invites: Vec<InviteInfo>,
    pub guests: Vec<GuestInfo>,
}

impl Guests {
    /// A new invite and its token, which only the caller ever sees.
    pub fn create(
        &self,
        permission: Permission,
        lifetime: Duration,
    ) -> Result<(Invite, String), InviteError> {
        self.inner.lock().unwrap().invites.create(
            &mut rand::rngs::OsRng,
            permission,
            lifetime,
            unix_now(),
        )
    }

    pub fn status(&self) -> Status {
        let now = unix_now();
        let mut inner = self.inner.lock().unwrap();
        inner.invites.prune(now);
        Status {
            streaming: inner.owner.is_some(),
            max_guests: MAX_GUESTS,
            invites: inner
                .invites
                .live(now)
                .map(|invite| InviteInfo {
                    id: invite.id.clone(),
                    permission: invite.permission,
                    created: invite.created,
                    expires: invite.expires,
                })
                .collect(),
            guests: inner
                .joined
                .iter()
                .map(|(id, joined)| GuestInfo {
                    id: *id,
                    name: joined.name.clone(),
                    permission: joined.permission,
                    invite: joined.invite.clone(),
                    since: joined.since,
                })
                .collect(),
        }
    }

    /// Withdraw an invite: it opens nothing from now on, and whoever joined with it is
    /// disconnected. False when there was no such invite.
    pub fn revoke(&self, id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let found = inner.invites.revoke(id);
        for joined in inner.joined.values().filter(|j| j.invite == id) {
            let _ = joined.leave.send(Leave::Revoked);
        }
        found
    }

    /// Disconnect a guest; its link still works. False when there was no such guest.
    pub fn kick(&self, id: u64) -> bool {
        let inner = self.inner.lock().unwrap();
        let Some(joined) = inner.joined.get(&id) else {
            return false;
        };
        let _ = joined.leave.send(Leave::Kicked);
        true
    }

    /// The owner's stream started; guests of a stream it replaced are disconnected. Returns the
    /// number [`owner_ended`](Self::owner_ended) takes.
    pub fn owner_started(&self, owner: Owner) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        let generation = inner.generation;
        for joined in inner.joined.values() {
            let _ = joined.leave.send(Leave::OwnerEnded);
        }
        inner.owner = Some((generation, owner));
        inner.keyframe_at = None;
        generation
    }

    /// The owner's stream `generation` ended: every guest is disconnected. Nothing happens for a
    /// stream that another replaced already.
    pub fn owner_ended(&self, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.owner.as_ref().is_none_or(|(g, _)| *g != generation) {
            return;
        }
        inner.owner = None;
        for joined in inner.joined.values() {
            let _ = joined.leave.send(Leave::OwnerEnded);
        }
    }

    /// Let in the guest whose page sent `token` from `peer`, unless that address sent too many
    /// wrong ones lately: a right token waits out the limit too.
    pub fn join(
        &self,
        peer: IpAddr,
        token: &str,
        name: &str,
        now: Instant,
    ) -> Result<Admission, JoinError> {
        self.join_at(peer, token, name, now, unix_now())
    }

    fn join_at(
        &self,
        peer: IpAddr,
        token: &str,
        name: &str,
        now: Instant,
        unix: u64,
    ) -> Result<Admission, JoinError> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.lookups.allowed(peer, now) {
            return Err(JoinError::RateLimited);
        }
        let Some((invite, permission)) = inner
            .invites
            .find(token, unix)
            .map(|invite| (invite.id.clone(), invite.permission))
        else {
            inner.lookups.failed(peer, now);
            return Err(JoinError::InvalidLink);
        };
        let Some((_, owner)) = inner.owner.clone() else {
            return Err(JoinError::NotStreaming);
        };
        let Some(slot) = (0..MAX_GUESTS).find(|s| inner.joined.values().all(|j| j.slot != *s))
        else {
            return Err(JoinError::Full);
        };
        inner.next_guest += 1;
        let id = inner.next_guest;
        let name = guest_name(name);
        let (leave, receiver) = unbounded_channel();
        inner.joined.insert(
            id,
            Joined {
                slot,
                name: name.clone(),
                permission,
                invite,
                since: unix,
                leave,
            },
        );
        Ok(Admission {
            id,
            slot,
            name,
            permission,
            owner,
            leave: receiver,
        })
    }

    /// A guest's stream ended, however it did.
    pub fn left(&self, id: u64) {
        self.inner.lock().unwrap().joined.remove(&id);
    }

    /// Ask the owner's encoder for a keyframe for a guest that lost its picture, unless a guest
    /// had one less than [`KEYFRAME_INTERVAL`] ago. False when it was not asked.
    pub fn keyframe(&self, now: Instant) -> bool {
        let session = {
            let mut inner = self.inner.lock().unwrap();
            if inner
                .keyframe_at
                .is_some_and(|at| now.saturating_duration_since(at) < KEYFRAME_INTERVAL)
            {
                return false;
            }
            let Some(session) = inner
                .owner
                .as_ref()
                .and_then(|(_, owner)| owner.session.upgrade())
            else {
                return false;
            };
            inner.keyframe_at = Some(now);
            session
        };
        // The host's own request, not the client's: it is no evidence about the owner's network.
        session.request_host_keyframe();
        true
    }

    /// Count a guest socket from `peer`; false when there are too many already.
    fn open_socket(&self, peer: IpAddr) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let total: usize = inner.sockets.values().sum();
        let count = inner.sockets.entry(peer_key(peer)).or_default();
        if total >= MAX_SOCKETS || *count >= MAX_SOCKETS_PER_PEER {
            return false;
        }
        *count += 1;
        true
    }

    fn close_socket(&self, peer: IpAddr) {
        let mut inner = self.inner.lock().unwrap();
        let key = peer_key(peer);
        if let Some(count) = inner.sockets.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                inner.sockets.remove(&key);
            }
        }
    }
}

/// The owner's stream while it runs, for guests to join; dropping it ends every guest's.
pub struct OwnerGuard {
    h: Shared,
    generation: u64,
}

impl OwnerGuard {
    pub fn new(h: &Shared, owner: Owner) -> Self {
        Self {
            generation: h.guests.owner_started(owner),
            h: h.clone(),
        }
    }
}

impl Drop for OwnerGuard {
    fn drop(&mut self) {
        self.h.guests.owner_ended(self.generation);
    }
}

/// A counted guest socket.
struct SocketSlot {
    h: Shared,
    peer: IpAddr,
}

impl Drop for SocketSlot {
    fn drop(&mut self) {
        self.h.guests.close_socket(self.peer);
    }
}

/// A joined guest, listed until its stream ends.
struct Seat {
    h: Shared,
    id: u64,
}

impl Drop for Seat {
    fn drop(&mut self) {
        self.h.guests.left(self.id);
    }
}

/// A guest's transport while it follows the owner's: dropping it stops the frames first, then
/// the transport, whose threads are joined off the async runtime.
struct Following {
    followers: Arc<Followers>,
    id: u64,
    handle: Option<MediaHandle>,
}

impl Drop for Following {
    fn drop(&mut self) {
        self.followers.remove(self.id);
        if let Some(handle) = self.handle.take() {
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => drop(runtime.spawn_blocking(move || drop(handle))),
                Err(_) => drop(handle),
            }
        }
    }
}

/// What a guest page sends.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum In {
    Join {
        token: String,
        #[serde(default)]
        name: String,
    },
    Answer {
        sdp: String,
    },
    Leave,
}

/// How a guest's socket ends: what the page is told, if anything.
#[derive(Debug, PartialEq)]
enum End {
    Left(Leave),
    Refused(&'static str, String),
    /// The page left or went away.
    Gone,
}

/// What the media threads report to the guest's socket.
enum Link {
    Connected,
    Closed,
}

/// The guest transport's hooks: its input goes to the owner's worker as this guest's, and a
/// lost picture asks for a keyframe within the guests' allowance.
struct Bridge {
    h: Shared,
    input: GuestHandle,
    link: UnboundedSender<Link>,
}

impl Bridge {
    fn event(&self, event: TransportEvent) {
        match event {
            TransportEvent::Connected => {
                let _ = self.link.send(Link::Connected);
                self.h.guests.keyframe(Instant::now());
            }
            TransportEvent::Closed => {
                let _ = self.link.send(Link::Closed);
            }
            TransportEvent::Control {
                data,
                binary: false,
            } => match std::str::from_utf8(&data).map(classify_control) {
                Ok(ControlMessage::KeyframeRequest) => {
                    self.h.guests.keyframe(Instant::now());
                }
                Ok(ControlMessage::GamepadAnnouncement) => {
                    self.input.gamepad(data, false, Instant::now());
                }
                _ => {}
            },
            TransportEvent::Gamepad { data, binary } => {
                self.input.gamepad(data, binary, Instant::now())
            }
            TransportEvent::Input { data, binary } => self.input.input(data, binary),
            TransportEvent::ChannelClosed { label } => self.input.channel_closed(&label),
            TransportEvent::Disconnected
            | TransportEvent::Reconnected
            | TransportEvent::ChannelOpen(_)
            | TransportEvent::ChannelRejected(_)
            | TransportEvent::Control { .. } => {}
        }
    }

    fn hooks(self: &Arc<Self>) -> Hooks {
        let (idr, event) = (self.clone(), self.clone());
        Hooks::new(
            move |_| {
                idr.h.guests.keyframe(Instant::now());
            },
            move |e| event.event(e),
        )
    }
}

/// Answer a guest page's upgrade. The caller checked its origin and that the WebRTC server is
/// on. `advertised` is the address the page was opened at (its `Host`), which a guest across a
/// port forward also reaches the media at.
pub fn upgrade(
    h: Shared,
    socket: WebSocketUpgrade,
    peer: SocketAddr,
    local: SocketAddr,
    advertised: Option<String>,
) -> Response {
    socket
        .protocols([PROTOCOL])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| connection(h, socket, peer, local, advertised))
}

async fn send(socket: &mut WebSocket, message: serde_json::Value) -> bool {
    socket
        .send(Message::Text(message.to_string().into()))
        .await
        .is_ok()
}

/// The page's next message; None when it closed or sent something that is not one.
async fn receive(socket: &mut WebSocket) -> Option<Result<In, ()>> {
    loop {
        match socket.recv().await? {
            Ok(Message::Text(text)) => {
                return Some(serde_json::from_str(text.as_str()).map_err(drop));
            }
            Ok(Message::Binary(_)) => return Some(Err(())),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

async fn connection(
    h: Shared,
    mut socket: WebSocket,
    peer: SocketAddr,
    local: SocketAddr,
    advertised: Option<String>,
) {
    let ip = peer.ip().to_canonical();
    let end = if h.guests.open_socket(ip) {
        let _slot = SocketSlot {
            h: h.clone(),
            peer: ip,
        };
        guest(
            &h,
            &mut socket,
            SocketAddr::new(ip, peer.port()),
            local,
            advertised,
        )
        .await
    } else {
        tracing::info!(%peer, "guest socket refused: too many are open");
        End::Refused(
            "busy",
            "Too many guest pages are open on this host; try again shortly.".into(),
        )
    };
    let said = match &end {
        End::Left(leave) => {
            let (reason, message) = leave.said();
            Some(json!({"type": "ended", "reason": reason, "message": message}))
        }
        End::Refused(code, message) => {
            Some(json!({"type": "error", "code": code, "message": message}))
        }
        End::Gone => None,
    };
    if let Some(said) = said {
        let _ = send(&mut socket, said).await;
    }
    let _ = socket.send(Message::Close(None)).await;
}

/// One guest, from its token to the end of its stream.
async fn guest(
    h: &Shared,
    socket: &mut WebSocket,
    peer: SocketAddr,
    local: SocketAddr,
    advertised: Option<String>,
) -> End {
    let refused = |code, message: &str| End::Refused(code, message.into());
    let (token, name) = match tokio::time::timeout(JOIN_TIMEOUT, receive(socket)).await {
        Ok(Some(Ok(In::Join { token, name }))) => (token, name),
        Ok(Some(_)) => return refused("invalid-message", "Send the invite token first."),
        Ok(None) => return End::Gone,
        Err(_) => return refused("timeout", "The page did not send its invite in time."),
    };
    let Admission {
        id,
        slot,
        name,
        permission,
        owner,
        mut leave,
    } = match h.guests.join(peer.ip(), &token, &name, Instant::now()) {
        Ok(admission) => admission,
        Err(error) => {
            tracing::info!(%peer, ?error, "guest refused");
            let (code, message) = error.said();
            return End::Refused(code, message);
        }
    };
    let _seat = Seat { h: h.clone(), id };
    tracing::info!(guest = id, %name, permission = permission.as_str(), %peer, "guest joined the console stream");

    let prepared = match prepare(h, &owner.spec, local.ip(), slot, advertised.as_deref()).await {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::error!(guest = id, error = %format!("{error:#}"), "guest stream could not be prepared");
            return End::Refused("internal", format!("{error:#}"));
        }
    };
    let video = &owner.spec.video;
    let offer = json!({
        "type": "offer",
        "sdp": prepared.offer.sdp,
        "name": name,
        "permission": permission,
        "codec": match video.codec {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
        },
        "width": video.width,
        "height": video.height,
        "fps": video.fps,
    });
    if !send(socket, offer).await {
        return End::Gone;
    }
    let answer = tokio::select! {
        biased;
        Some(why) = leave.recv() => return End::Left(why),
        answer = tokio::time::timeout(ANSWER_TIMEOUT, receive(socket)) => match answer {
            Ok(Some(Ok(In::Answer { sdp }))) => sdp,
            Ok(Some(Ok(In::Leave))) | Ok(None) => return End::Gone,
            Ok(Some(_)) => return refused("invalid-message", "Answer the offer."),
            Err(_) => return refused("timeout", "The page did not answer the offer in time."),
        },
    };

    let (link, mut linked) = unbounded_channel();
    let bridge = Arc::new(Bridge {
        h: h.clone(),
        input: owner.input.guest(id, permission),
        link,
    });
    let started = {
        let (h, owner, bridge) = (h.clone(), owner.clone(), bridge.clone());
        tokio::task::spawn_blocking(move || {
            start(&h, prepared, &answer, &owner, permission, peer, &bridge)
        })
        .await
        .unwrap_or_else(|error| {
            Err(End::Refused(
                "internal",
                format!("starting the guest's stream panicked: {error}"),
            ))
        })
    };
    drop(bridge);
    let handle = match started {
        Ok(handle) => handle,
        Err(end) => return end,
    };
    owner.followers.add(id, handle.feed());
    let _following = Following {
        followers: owner.followers.clone(),
        id,
        handle: Some(handle),
    };
    h.guests.keyframe(Instant::now());

    let connect = tokio::time::sleep(CONNECT_TIMEOUT);
    tokio::pin!(connect);
    let mut connected = false;
    loop {
        tokio::select! {
            biased;
            Some(why) = leave.recv() => {
                tracing::info!(guest = id, ?why, "guest disconnected by the host");
                return End::Left(why);
            }
            event = linked.recv() => match event {
                Some(Link::Connected) if !connected => {
                    connected = true;
                    tracing::info!(guest = id, "guest stream connected");
                    if !send(socket, json!({"type": "streaming"})).await {
                        return End::Gone;
                    }
                }
                Some(Link::Connected) => {}
                Some(Link::Closed) | None => {
                    return refused("connection-lost", "The connection to the host was lost.");
                }
            },
            _ = &mut connect, if !connected => {
                tracing::warn!(guest = id, "guest stream did not connect in time");
                return End::Refused("timeout", format!("The stream did not connect. The host's UDP port {} must reach this device.", prepared_port(h, slot)));
            }
            message = receive(socket) => match message {
                Some(Ok(In::Leave)) | None => return End::Gone,
                Some(_) => {}
            },
        }
    }
}

fn prepared_port(h: &Shared, slot: u16) -> u32 {
    u32::from(h.config.read().unwrap().webrtc_media_port()) + 1 + u32::from(slot)
}

/// The guest's socket, bound and offered.
struct Prepared {
    socket: UdpSocket,
    local: SocketAddr,
    rtc: str0m::Rtc,
    offer: sdp::Offer,
}

/// The address in a `Host` header, without the port.
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
            name
        }
        _ => host,
    }
}

/// The address the page was opened at, when it is another than the socket's: a router's that
/// forwards to it. None for the socket's own, another family, or a name that does not resolve.
async fn advertised_address(host: &str, local: IpAddr) -> Option<IpAddr> {
    let name = host_name(host);
    let address = match name.parse::<IpAddr>() {
        Ok(address) => address,
        Err(_) => tokio::time::timeout(Duration::from_secs(2), tokio::net::lookup_host((name, 0)))
            .await
            .ok()?
            .ok()?
            .map(|a| a.ip())
            .find(|ip| ip.is_ipv4() == local.is_ipv4())?,
    }
    .to_canonical();
    (address != local
        && address.is_ipv4() == local.is_ipv4()
        && !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast())
    .then_some(address)
}

/// Bind the guest's media port on the address its page reached and build its offer for the
/// owner's stream.
async fn prepare(
    h: &Shared,
    spec: &OfferSpec,
    local: IpAddr,
    slot: u16,
    advertised: Option<&str>,
) -> Result<Prepared> {
    let base = h.config.read().unwrap().webrtc_media_port();
    let port = base
        .checked_add(1 + slot)
        .context("no UDP port is left above webrtc_media_port for guests")?;
    let deadline = tokio::time::Instant::now() + BIND_WAIT;
    let socket = loop {
        match media::bind_socket((local, port).into()) {
            Ok(socket) => break socket,
            Err(error)
                if tokio::time::Instant::now() < deadline
                    && error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error.context(format!("binding the guest media port {port}"))),
        }
    };
    let bound = socket.local_addr().context("reading the media address")?;
    let mut rtc = sdp::rtc_config(spec)?.build(Instant::now());
    rtc.add_local_candidate(str0m::Candidate::host(bound, "udp").context("the host candidate")?);
    if let Some(host) = advertised
        && let Some(public) = advertised_address(host, bound.ip()).await
    {
        // Through a port forward the guest reaches the router's address; the router hands its
        // checks to this socket, which answers them as from its own.
        rtc.add_local_candidate(
            str0m::Candidate::host((public, port).into(), "udp")
                .context("the advertised candidate")?,
        );
    }
    let offer = sdp::create_offer(&mut rtc, spec)?;
    Ok(Prepared {
        socket,
        local: bound,
        rtc,
        offer,
    })
}

/// Accept the guest's answer and start its transport. Blocking: it routes and starts threads.
fn start(
    h: &Shared,
    prepared: Prepared,
    answer: &str,
    owner: &Owner,
    permission: Permission,
    peer: SocketAddr,
    bridge: &Arc<Bridge>,
) -> Result<MediaHandle, End> {
    let Prepared {
        socket,
        local,
        mut rtc,
        offer,
    } = prepared;
    let (ids, answered) =
        sdp::accept_answer(&mut rtc, offer, answer, &owner.spec).map_err(|error| {
            End::Refused(
                "unsupported",
                format!("This browser cannot play the host's stream: {error:#}"),
            )
        })?;
    if answered.audio_layout != owner.audio_layout {
        return Err(End::Refused(
            "unsupported",
            "This browser answered with another sound layout than the host's stream has.".into(),
        ));
    }
    let link = butterpollo_windows::net::routed_link(peer);
    let (configured_kbps, paced, loss_permille) = {
        let config = h.config.read().unwrap();
        (
            config.integer("pacing_max_bitrate_kbps", 0),
            config.webrtc_pacing(),
            config.webrtc_loss_permille(),
        )
    };
    let options = TransportOptions {
        pacing: if paced {
            Pacing::Paced {
                configured_kbps,
                link_bps: link.bps,
                wireless: link.wireless,
            }
        } else {
            Pacing::Off
        },
        loss_permille,
        ..TransportOptions::default()
    };
    let handle = MediaHandle::spawn(
        socket,
        local,
        media::Session {
            rtc,
            ids,
            spec: &owner.spec,
            // The `input` channel is the keyboard and mouse: only with that permission.
            authenticated: Arc::new(AtomicBool::new(permission.keyboard_mouse())),
        },
        options,
        bridge.hooks(),
    )
    .map_err(|error| End::Refused("internal", format!("{error:#}")))?;
    bridge.input.control(handle.control_sender());
    Ok(handle)
}

#[cfg(test)]
mod tests;
