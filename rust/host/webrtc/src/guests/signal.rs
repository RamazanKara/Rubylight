//! Guest WebSocket signalling and media session setup for browser guests.

use super::{
    ANSWER_TIMEOUT, Admission, BIND_WAIT, CONNECT_TIMEOUT, Guests, JOIN_TIMEOUT, Leave,
    MAX_MESSAGE, Owner, PROTOCOL,
    guards::{Seat, SocketSlot},
    keyframe_backoff::KeyframeBackoff,
};
use crate::{
    input::GuestHandle,
    media::{
        self, Followers, Hooks, IdrReason, MediaHandle, Pacing, TransportEvent, TransportOptions,
    },
    sdp::{self, OfferSpec, VideoCodec},
};
use anyhow::{Context, Result};
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
};
use butterpollo_session::webrtc::{
    guests::Permission,
    protocol::{ControlMessage, classify_control},
};
use butterpollo_stream::{
    network::{media_address, this_pc_address},
    state::{Session, Shared},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{Arc, Mutex, Weak, atomic::AtomicBool},
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

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
    guests: Arc<Guests>,
    /// None for a view-only guest, whose input goes nowhere.
    input: Option<GuestHandle>,
    /// The owner's session, whose encoder makes the keyframes.
    session: Weak<Session>,
    link: UnboundedSender<Link>,
    /// This guest's own wait between keyframes, within the guests' shared allowance.
    backoff: Mutex<KeyframeBackoff>,
}

impl Bridge {
    /// A keyframe for this guest, within its backoff; a connection or a return after an
    /// outage needs a first picture and is not held back by it.
    fn keyframe(&self, reason: IdrReason) {
        let now = Instant::now();
        let fresh = matches!(reason, IdrReason::Connected | IdrReason::Reconnected);
        let mut backoff = self.backoff.lock().unwrap_or_else(|e| e.into_inner());
        backoff.request(now, fresh, || self.guests.keyframe(&self.session, now));
    }

    fn event(&self, event: TransportEvent) {
        let input = self.input.as_ref();
        match event {
            TransportEvent::Connected => {
                let _ = self.link.send(Link::Connected);
                self.keyframe(IdrReason::Connected);
            }
            TransportEvent::Closed => {
                let _ = self.link.send(Link::Closed);
            }
            TransportEvent::Control {
                data,
                binary: false,
            } => match std::str::from_utf8(&data).map(classify_control) {
                Ok(ControlMessage::KeyframeRequest) => self.keyframe(IdrReason::Pli),
                Ok(ControlMessage::GamepadAnnouncement) => {
                    if let Some(input) = input {
                        input.gamepad(data, false, Instant::now());
                    }
                }
                _ => {}
            },
            TransportEvent::Gamepad { data, binary } => {
                if let Some(input) = input {
                    input.gamepad(data, binary, Instant::now());
                }
            }
            TransportEvent::Input { data, binary } => {
                if let Some(input) = input {
                    input.input(data, binary);
                }
            }
            TransportEvent::ChannelClosed { label } => {
                if let Some(input) = input {
                    input.channel_closed(&label);
                }
            }
            TransportEvent::Disconnected
            | TransportEvent::Reconnected
            | TransportEvent::ChannelOpen(_)
            | TransportEvent::ChannelRejected(_)
            | TransportEvent::Control { .. } => {}
        }
    }

    fn hooks(self: &Arc<Self>) -> Hooks {
        let (idr, event) = (self.clone(), self.clone());
        Hooks::new(move |reason| idr.keyframe(reason), move |e| event.event(e))
    }
}

/// Answer a guest page's upgrade. The caller checked its origin and that the WebRTC server is
/// on.
pub fn upgrade(
    h: Shared,
    guests: Arc<Guests>,
    socket: WebSocketUpgrade,
    peer: SocketAddr,
    local: SocketAddr,
) -> Response {
    socket
        .protocols([PROTOCOL])
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_upgrade(move |socket| connection(h, guests, socket, peer, local))
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
    guests: Arc<Guests>,
    mut socket: WebSocket,
    peer: SocketAddr,
    local: SocketAddr,
) {
    let ip = peer.ip().to_canonical();
    let end = match guests.open_socket(ip, Instant::now()) {
        Ok(()) => {
            let mut slot = SocketSlot {
                guests: guests.clone(),
                peer: ip,
                counted: true,
            };
            guest(
                &h,
                &guests,
                &mut socket,
                SocketAddr::new(ip, peer.port()),
                local,
                &mut slot,
            )
            .await
        }
        Err(error) => {
            tracing::info!(%peer, ?error, "guest socket refused");
            let (code, message) = error.said();
            End::Refused(code, message)
        }
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

/// The page's end of a failure whose detail stays in the host's log.
const COULD_NOT_START: &str = "The host could not start your stream.";

/// One guest, from its token to the end of its stream.
async fn guest(
    h: &Shared,
    guests: &Arc<Guests>,
    socket: &mut WebSocket,
    peer: SocketAddr,
    local: SocketAddr,
    slot: &mut SocketSlot,
) -> End {
    let refused = |code, message: &str| End::Refused(code, message.into());
    let (token, name) = match tokio::time::timeout(JOIN_TIMEOUT, receive(socket)).await {
        Ok(Some(Ok(In::Join { token, name }))) => (token, name),
        Ok(Some(_)) => {
            guests.unanswered(peer.ip(), Instant::now());
            return refused("invalid-message", "Send the invite token first.");
        }
        Ok(None) => return End::Gone,
        Err(_) => {
            guests.unanswered(peer.ip(), Instant::now());
            return refused("timeout", "The page did not send its invite in time.");
        }
    };
    let Admission {
        id,
        slot: seat,
        name,
        permission,
        owner,
        mut leave,
        address,
    } = match guests.join(peer.ip(), &token, &name, Instant::now()) {
        Ok(admission) => admission,
        Err(error) => {
            tracing::info!(%peer, ?error, "guest refused");
            let (code, message) = error.said();
            return End::Refused(code, message);
        }
    };
    slot.joined();
    let _seat = Seat {
        guests: guests.clone(),
        id,
    };
    tracing::info!(guest = id, %name, permission = permission.as_str(), %peer, "guest joined the console stream");

    let media = media_address(local.ip(), owner.bind, this_pc_address);
    let prepared = match prepare(h, &owner.spec, media, seat, address.as_deref()).await {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::error!(guest = id, error = %format!("{error:#}"), "guest stream could not be prepared");
            return refused("internal", COULD_NOT_START);
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
        guests: guests.clone(),
        input: owner.input.guest(id, permission),
        session: owner.session.clone(),
        link,
        backoff: Mutex::default(),
    });
    let started = {
        let (h, owner, bridge) = (h.clone(), owner.clone(), bridge.clone());
        tokio::task::spawn_blocking(move || {
            start(&h, prepared, &answer, &owner, permission, peer, &bridge)
        })
        .await
        .unwrap_or_else(|error| {
            tracing::error!(guest = id, %error, "starting a guest's stream panicked");
            Err(End::Refused("internal", COULD_NOT_START.into()))
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
    // Keyframe requests wait until ICE/DTLS connects (`IdrReason::Connected` /
    // `TransportEvent::Connected`), so joining never burns `KEYFRAME_INTERVAL`
    // while `write_video` would still drop the IDR.

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
                return End::Refused("timeout", format!("The stream did not connect. The host's UDP port {} must reach this device.", prepared_port(h, seat)));
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

/// The host in an address the owner typed (`name`, `name:port`, `[v6]:port`), without the port.
pub(super) fn host_name(host: &str) -> &str {
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

/// The address the owner put in the link, when it is another than the socket's: a router's that
/// forwards to it. None for the socket's own, another family, or a name that does not resolve.
/// Only the owner's address is looked up, never one a guest sent.
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
/// owner's stream, offered at `advertised` too, the owner's address in the link.
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
    tracing::info!(media = %bound, advertised = ?advertised, "guest offer's host candidate");
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
            tracing::info!(%peer, error = %format!("{error:#}"), "a guest's answer was refused");
            End::Refused(
                "unsupported",
                "This browser cannot play the host's stream.".into(),
            )
        })?;
    if answered.audio_layout != owner.audio_layout {
        return Err(End::Refused(
            "unsupported",
            "This browser answered with another sound layout than the host's stream has.".into(),
        ));
    }
    let link = butterpollo_win_io::net::routed_link(peer);
    let (configured_kbps, paced, loss_permille) = {
        let config = h.config.read().unwrap();
        (
            config.manual_pacing_kbps().unwrap_or(0),
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
    .map_err(|error| {
        tracing::error!(%peer, error = %format!("{error:#}"), "a guest's transport did not start");
        End::Refused("internal", COULD_NOT_START.into())
    })?;
    if let Some(input) = &bridge.input {
        input.control(handle.control_sender());
    }
    Ok(handle)
}
