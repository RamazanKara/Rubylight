//! The str0m media transport: a Sans-IO [`Engine`] around one `Rtc`, and the thread that gives it
//! a UDP socket (`docs/webrtc-design.md`, "Library" and "Media path").
//!
//! One ICE-lite UDP socket carries everything. The encoder thread never blocks on it: `submit_*`
//! only queues, and a full queue drops the frame and asks for an IDR through the hook. Video is
//! stamped with the capture instant, so the receiver's jitter buffer sees the host's real frame
//! spacing, and the playout-delay extension asks it for no buffering at all.

use super::sdp::{OfferSpec, SessionIds, VideoCodec};
use anyhow::{Context, Result};
pub use butterpollo_stream::webrtc_bridge::{AudioPacket, Submit, WebRtcAudioSink};
use socket2::{Domain, Protocol as SockProtocol, Socket, Type};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use str0m::Rtc;

mod engine;
mod handle;
mod pacing;
mod worker;

pub use engine::{Engine, strip_av1_temporal_delimiters};
pub use handle::{AudioSender, ControlSender, Feed, Followers, MediaHandle};
pub use pacing::{Batch, SendQueue};

#[cfg(test)]
use engine::is_media_rtp;
#[cfg(test)]
use handle::{Command, Counters, Shared};
#[cfg(test)]
use worker::{
    MAX_COMMANDS_PER_PASS, RECV_ERROR_BACKOFF, RECV_ERRORS_BEFORE_BACKOFF, drain_commands,
    enforce_backlog, handle_command, note_channel_send, receive_loop, recv_error_backoff, send_due,
    start_threads,
};

/// Socket send buffer: a 4K keyframe is a few hundred KB sent in one pacing pass.
pub const SEND_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const RECV_BUFFER_BYTES: usize = 1024 * 1024;
/// Room for any UDP datagram, so an oversized one is read whole and dropped by the parser
/// instead of failing the receive (Windows reports WSAEMSGSIZE for a short buffer).
const RECV_DATAGRAM_BYTES: usize = 64 * 1024;
/// Queued frames between the encoder and the media thread. At 60+ fps this is a few tens of
/// milliseconds; more would only delay the picture.
pub const DEFAULT_VIDEO_QUEUE: usize = 4;
pub const DEFAULT_AUDIO_QUEUE: usize = 64;
/// Received datagrams waiting for the media thread: a couple of milliseconds of a fast uplink.
/// More means the thread is stalled, and the overflow is dropped, newest first.
pub const DEFAULT_DATAGRAM_QUEUE: usize = 2048;
/// How often a stalled encoder (waiting for a keyframe) is asked again.
const IDR_REQUEST_INTERVAL: Duration = Duration::from_millis(250);
/// An ICE path that stays down this long ends the session. A shorter outage is a network
/// handover or a Wi-Fi drop of a few seconds, and the session resumes with a keyframe when the
/// path comes back. A browser waits longer before it gives up on its connection.
pub const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How far behind schedule a late wake may send back to back to catch up; a longer stall earns
/// no more than this, so the pacer never releases a burst.
const CATCH_UP_CREDIT: Duration = Duration::from_millis(4);
/// The high-resolution timer returns up to this much before its deadline (its learned lead is
/// at most 1 ms), and that early send is on schedule.
const EARLY_SEND: Duration = Duration::from_millis(1);
/// Paced media older than this at the pacing rate is hopeless: it is dropped and a keyframe
/// requested rather than delivered late. Never less than two frames of the biggest recent frame.
const MAX_PACED_BACKLOG: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// Hand every packet to the socket as soon as str0m produces it.
    Off,
    /// 1 ms groups at the route's rate, `network_pacing::rate_bps` with the frame's own wire
    /// demand as the needed rate. `link_bps` is the routed link (0 when unknown): the pace is
    /// capped at 4/5 of it however much a frame needs. B5 compares paced with unpaced.
    Paced {
        /// The user's pacing cap in kbps, or 0/negative for none.
        configured_kbps: i64,
        link_bps: u64,
        wireless: bool,
    },
}

impl Pacing {
    /// No configured cap and an unknown, wired route: 800 Mbps.
    pub const DEFAULT: Self = Self::Paced {
        configured_kbps: 0,
        link_bps: 0,
        wireless: false,
    };
}

#[derive(Clone, Copy, Debug)]
pub struct TransportOptions {
    pub pacing: Pacing,
    pub video_queue: usize,
    pub audio_queue: usize,
    pub datagram_queue: usize,
    /// Video datagrams dropped at the socket per thousand, as a lossy link does; 0 is off. A
    /// measurement switch (`webrtc_loss_permille`), for the NACK and RTX comparison.
    pub loss_permille: u32,
}

impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            pacing: Pacing::DEFAULT,
            video_queue: DEFAULT_VIDEO_QUEUE,
            audio_queue: DEFAULT_AUDIO_QUEUE,
            datagram_queue: DEFAULT_DATAGRAM_QUEUE,
            loss_permille: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdrReason {
    Pli,
    Fir,
    Connected,
    /// The ICE path came back after a disconnect: the receiver missed frames.
    Reconnected,
    QueueFull,
    /// The pacer fell too far behind and dropped what it held.
    PacerBacklog,
    WaitingForKeyframe,
    WriteFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    Connected,
    Closed,
    /// The ICE path is down. The session ends after [`DISCONNECT_TIMEOUT`] unless it comes back.
    Disconnected,
    /// The ICE path came back; a keyframe has been requested.
    Reconnected,
    /// A channel the host expects is open.
    ChannelOpen(String),
    /// A channel that was open is gone (the client closed it, or the connection ended).
    ChannelClosed {
        label: String,
    },
    /// A channel the client opened that the host refuses (unknown label, or `input` before the
    /// session is authenticated). It has been closed.
    ChannelRejected(String),
    Control {
        data: Vec<u8>,
        binary: bool,
    },
    Gamepad {
        data: Vec<u8>,
        binary: bool,
    },
    Input {
        data: Vec<u8>,
        binary: bool,
    },
}

/// Called from the media thread (PLI, FIR, connected) and from the encoder thread (queue full),
/// so it must return quickly.
#[derive(Clone)]
pub struct Hooks {
    pub idr: Arc<dyn Fn(IdrReason) + Send + Sync>,
    pub event: Arc<dyn Fn(TransportEvent) + Send + Sync>,
}

impl Hooks {
    pub fn new(
        idr: impl Fn(IdrReason) + Send + Sync + 'static,
        event: impl Fn(TransportEvent) + Send + Sync + 'static,
    ) -> Self {
        Self {
            idr: Arc::new(idr),
            event: Arc::new(event),
        }
    }
}

pub struct VideoFrame {
    /// One access unit: Annex B for H.264 and HEVC, low-overhead OBUs for AV1.
    pub data: FrameData,
    pub capture: Instant,
    pub keyframe: bool,
}

/// An access unit's bytes. str0m packetizes from an `Arc<[u8]>`, so a frame several transports
/// send needs no copy per transport.
pub enum FrameData {
    /// Only this transport sends it: moved here, and copied into str0m once, by the media thread.
    Owned(Vec<u8>),
    /// Every transport of a session with guests sends it ([`Followers`]): copied once, before any
    /// of them. AV1's temporal delimiters are out already.
    Shared(Arc<[u8]>),
}

impl FrameData {
    pub fn len(&self) -> usize {
        match self {
            Self::Owned(data) => data.len(),
            Self::Shared(data) => data.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The buffer every transport sends from, ready for `codec`.
    pub(super) fn share(self, codec: VideoCodec) -> Result<Arc<[u8]>> {
        Ok(match (self, codec) {
            (Self::Owned(data), VideoCodec::Av1) => strip_av1_temporal_delimiters(data)?.into(),
            (Self::Owned(data), _) => data.into(),
            (Self::Shared(data), _) => data,
        })
    }
}

impl From<Vec<u8>> for FrameData {
    fn from(data: Vec<u8>) -> Self {
        Self::Owned(data)
    }
}

/// What str0m wants on the wire and how.
#[derive(Debug)]
pub struct Outgoing {
    pub destination: SocketAddr,
    pub data: Vec<u8>,
    /// Video RTP, which the pacer spreads. Audio, STUN, DTLS, SCTP and RTCP go out at once: a
    /// 200-byte Opus packet must not queue behind a keyframe.
    pub paced: bool,
}

impl AsRef<[u8]> for Outgoing {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

/// What became of a datagram handed to [`Engine::handle_datagram`]. None of these ends the
/// session: one bad or foreign datagram is dropped and counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Datagram {
    Handled,
    /// Not for this session (unparsable, or from a source that isn't the peer's).
    Rejected,
    /// Accepted as the peer's but str0m refused it (a failed SRTP or DTLS check, say).
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransportStats {
    pub video_frames: u64,
    pub video_bytes: u64,
    pub video_dropped: u64,
    pub audio_packets: u64,
    pub audio_dropped: u64,
    pub datagrams_sent: u64,
    pub bytes_sent: u64,
    pub send_errors: u64,
    pub idr_requests: u64,
    pub datagrams_received: u64,
    pub datagrams_dropped: u64,
    pub datagrams_rejected: u64,
    pub datagram_errors: u64,
    pub recv_errors: u64,
    pub paced_dropped: u64,
    pub channel_sent: u64,
    pub channel_dropped: u64,
    pub simulated_losses: u64,
}

/// A bound UDP socket for the media. Bind first so the offer can carry the real port.
pub fn bind_socket(addr: SocketAddr) -> Result<UdpSocket> {
    let domain = if addr.is_ipv6() {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::DGRAM, Some(SockProtocol::UDP))
        .context("creating the media socket")?;
    socket
        .set_send_buffer_size(SEND_BUFFER_BYTES)
        .context("sizing the media send buffer")?;
    let _ = socket.set_recv_buffer_size(RECV_BUFFER_BYTES);
    socket
        .bind(&addr.into())
        .with_context(|| format!("binding the media socket to {addr}"))?;
    Ok(socket.into())
}

/// What `sdp::accept_answer` leaves behind, handed to the media thread.
pub struct Session<'a> {
    pub rtc: Rtc,
    pub ids: SessionIds,
    pub spec: &'a OfferSpec,
    /// Whether the client may open the `input` channel; [`MediaHandle::set_authenticated`] flips it.
    pub authenticated: Arc<AtomicBool>,
}

#[cfg(any(test, feature = "test-support"))]
pub use crate::test_support::real_handle;

#[cfg(test)]
pub(crate) mod tests;
