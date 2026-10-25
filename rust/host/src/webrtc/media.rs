//! The str0m media transport: a Sans-IO [`Engine`] around one `Rtc`, and the thread that gives it
//! a UDP socket (`docs/webrtc-design.md`, "Library" and "Media path").
//!
//! One ICE-lite UDP socket carries everything. The encoder thread never blocks on it: `submit_*`
//! only queues, and a full queue drops the frame and asks for an IDR through the hook. Video is
//! stamped with the capture instant, so the receiver's jitter buffer sees the host's real frame
//! spacing, and the playout-delay extension asks it for no buffering at all.
use super::sdp::{
    AUDIO_PT, CONTROL_LABEL, GAMEPAD_LABEL, INPUT_LABEL, OfferSpec, SessionIds, VIDEO_PT,
    VideoCodec,
};
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_core::{audio::OpusLayout, network_pacing};
use butterpollo_windows::timing::{Signal, Timer};
use socket2::{Domain, Protocol as SockProtocol, Socket, Type};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use str0m::IceConnectionState;
use str0m::channel::{ChannelId, Reliability};
use str0m::media::{Frequency, MediaTime, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Event, Input, Output, Rtc};

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
/// The UDP payload size str0m targets (`DATAGRAM_MTU_TARGET`), for the pacer's frame maths.
const DATAGRAM_TARGET: usize = 1150;
const RTP_PAYLOAD_TARGET: usize = DATAGRAM_TARGET - 50;
/// An ICE path that stays down this long ends the session. A shorter outage is a network
/// handover, and the session resumes with a keyframe when the path comes back.
pub const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(3);
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
    pub data: Vec<u8>,
    pub capture: Instant,
    pub keyframe: bool,
}

pub struct AudioPacket {
    /// One Opus packet (or one multistream packet), sent as is.
    pub data: Vec<u8>,
    /// When the audio thread handed the packet over. Only str0m's NTP pairing uses it.
    pub capture: Instant,
    /// Where this packet starts in the audio thread's own stream, in 48 kHz samples: every packet
    /// the thread encoded advances it by its length, dropped or not, and audio Windows lost
    /// advances it too. The RTP clock follows it, not wall time.
    pub position: u64,
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

/// RTP, not RTCP, STUN or DTLS (RFC 7983 demultiplexing, rtcp-mux).
fn is_media_rtp(data: &[u8]) -> bool {
    data.len() > 12 && (128..192).contains(&data[0]) && !(192..224).contains(&data[1])
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

fn ticks(since: Duration, hz: u64) -> u64 {
    (since.as_nanos() * u128::from(hz) / 1_000_000_000) as u64
}

fn read_leb128(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (i, byte) in data.iter().take(8).enumerate() {
        value |= usize::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Drop the temporal-delimiter OBUs: RFC 9628 says not to send them, and the receiver adds its
/// own. Other OBUs pass byte for byte.
pub fn strip_av1_temporal_delimiters(data: &[u8]) -> Result<Cow<'_, [u8]>> {
    const OBU_TEMPORAL_DELIMITER: u8 = 2;
    let mut keep: Vec<(usize, usize)> = Vec::new();
    let mut found = false;
    let mut at = 0;
    while at < data.len() {
        let header = data[at];
        if header & 0x80 != 0 {
            bail!("AV1 OBU with the forbidden bit set at byte {at}");
        }
        let obu_type = (header >> 3) & 0x0f;
        let header_len = 1 + usize::from(header & 0x04 != 0);
        let (payload, size_len) = if header & 0x02 != 0 {
            let size = data.get(at + header_len..).and_then(read_leb128);
            size.ok_or_else(|| anyhow!("AV1 OBU size is cut off at byte {at}"))?
        } else {
            (data.len().saturating_sub(at + header_len), 0)
        };
        let end = at + header_len + size_len + payload;
        if end > data.len() {
            bail!(
                "AV1 OBU at byte {at} runs {} bytes past the access unit",
                end - data.len()
            );
        }
        if obu_type == OBU_TEMPORAL_DELIMITER {
            found = true;
        } else {
            keep.push((at, end));
        }
        at = end;
    }
    if !found {
        return Ok(Cow::Borrowed(data));
    }
    Ok(Cow::Owned(
        keep.iter()
            .flat_map(|&(a, b)| data[a..b].iter().copied())
            .collect(),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Control,
    Gamepad,
    Input,
}

/// One session's Rtc with no I/O of its own: datagrams and time go in, datagrams and events come
/// out. Tests drive it with a virtual clock; [`MediaHandle`] drives it from a socket.
pub struct Engine {
    rtc: Rtc,
    ids: SessionIds,
    codec: VideoCodec,
    /// The advertised host candidate, which incoming datagrams are addressed to.
    local: SocketAddr,
    start: Instant,
    authenticated: Arc<AtomicBool>,
    input: Option<ChannelId>,
    control_open: bool,
    gamepad_open: bool,
    outgoing: VecDeque<Outgoing>,
    events: Vec<TransportEvent>,
    idr: Vec<IdrReason>,
    connected: bool,
    closed: bool,
    /// Since when the ICE path has been down, while it is.
    disconnected_since: Option<Instant>,
    /// Wire bytes of the biggest frame written since [`Engine::take_frame_wire_bytes`].
    frame_wire_bytes: usize,
    rejected: u64,
    failed: u64,
    /// RTP time of audio position 0, 48 kHz ticks since `start`: set from the capture instant of
    /// the first packet written, then never again, so wall time cannot move the audio timeline.
    audio_base: Option<u64>,
}

impl Engine {
    /// `start` is the RTP timeline's zero: capture instants map to 90 kHz (video) and 48 kHz
    /// (audio) ticks since it.
    pub fn new(
        rtc: Rtc,
        ids: SessionIds,
        spec: &OfferSpec,
        local: SocketAddr,
        start: Instant,
        authenticated: Arc<AtomicBool>,
    ) -> Self {
        Self {
            rtc,
            ids,
            codec: spec.video.codec,
            local,
            start,
            authenticated,
            input: None,
            control_open: false,
            gamepad_open: false,
            outgoing: VecDeque::new(),
            events: Vec::new(),
            idr: Vec::new(),
            connected: false,
            closed: false,
            disconnected_since: None,
            frame_wire_bytes: 0,
            rejected: 0,
            failed: 0,
            audio_base: None,
        }
    }

    /// Media can flow: connected, and the ICE path is up.
    pub fn is_connected(&self) -> bool {
        self.connected && self.disconnected_since.is_none() && self.rtc.is_alive()
    }

    /// The ICE path is down and the session is waiting for it ([`DISCONNECT_TIMEOUT`]).
    pub fn is_disconnected(&self) -> bool {
        self.disconnected_since.is_some()
    }

    pub fn is_closed(&self) -> bool {
        self.closed || !self.rtc.is_alive()
    }

    /// Wire bytes of the biggest frame written since the last call, for the pacer.
    pub fn take_frame_wire_bytes(&mut self) -> usize {
        std::mem::take(&mut self.frame_wire_bytes)
    }

    /// Datagrams dropped as not the peer's, and ones str0m refused, since the start.
    pub fn dropped_datagrams(&self) -> (u64, u64) {
        (self.rejected, self.failed)
    }

    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.outgoing.drain(..).collect()
    }

    pub fn take_events(&mut self) -> Vec<TransportEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn take_idr_requests(&mut self) -> Vec<IdrReason> {
        std::mem::take(&mut self.idr)
    }

    /// A datagram that arrived on the socket. The port is public, so anyone can send to it:
    /// str0m is only asked to handle what it says is for this session (`Rtc::accepts`), and
    /// whatever it still refuses is dropped, counted and logged, never fatal.
    pub fn handle_datagram(&mut self, now: Instant, from: SocketAddr, data: &[u8]) -> Datagram {
        let receive = match Receive::new(Protocol::Udp, from, self.local, data) {
            Ok(receive) => receive,
            Err(e) => {
                self.note_rejected(from, &format!("unparsable: {e}"));
                return Datagram::Rejected;
            }
        };
        let input = Input::Receive(now, receive);
        if !self.rtc.accepts(&input) {
            self.note_rejected(from, "not from the peer");
            return Datagram::Rejected;
        }
        match self.rtc.handle_input(input) {
            Ok(()) => Datagram::Handled,
            Err(e) => {
                if self.failed.is_multiple_of(100) {
                    tracing::warn!(
                        "webrtc: dropped a datagram from {from} ({} so far): {e}",
                        self.failed + 1
                    );
                }
                self.failed += 1;
                Datagram::Failed
            }
        }
    }

    fn note_rejected(&mut self, from: SocketAddr, why: &str) {
        if self.rejected.is_multiple_of(100) {
            tracing::debug!(
                "webrtc: ignored a datagram from {from} ({} so far): {why}",
                self.rejected + 1
            );
        }
        self.rejected += 1;
    }

    /// Advance the clock and collect everything str0m has ready. Returns when it wants to be
    /// called again.
    pub fn advance(&mut self, now: Instant) -> Result<Instant> {
        self.rtc
            .handle_input(Input::Timeout(now))
            .map_err(|e| anyhow!("webrtc transport failed: {e}"))?;
        let wake = self.poll(now)?;
        let Some(since) = self.disconnected_since else {
            return Ok(wake);
        };
        let deadline = since + DISCONNECT_TIMEOUT;
        if now < deadline {
            return Ok(wake.min(deadline));
        }
        if !self.closed {
            tracing::warn!(
                "webrtc: the connection did not come back within {:?}, closing",
                DISCONNECT_TIMEOUT
            );
            self.closed = true;
        }
        Ok(wake)
    }

    fn poll(&mut self, now: Instant) -> Result<Instant> {
        loop {
            match self
                .rtc
                .poll_output()
                .map_err(|e| anyhow!("webrtc transport failed: {e}"))?
            {
                Output::Timeout(at) => return Ok(at.max(now)),
                Output::Transmit(t) => {
                    let data: Vec<u8> = t.contents.into();
                    // Only video waits for the pacer. The payload type sits in the clear.
                    let paced = is_media_rtp(&data) && data[1] & 0x7f != AUDIO_PT;
                    self.outgoing.push_back(Outgoing {
                        destination: t.destination,
                        data,
                        paced,
                    });
                }
                Output::Event(event) => self.on_event(now, event),
            }
        }
    }

    fn on_event(&mut self, now: Instant, event: Event) {
        match event {
            Event::Connected => {
                self.connected = true;
                self.events.push(TransportEvent::Connected);
                // Nothing was sent before this point, so the picture starts with a keyframe.
                self.idr.push(IdrReason::Connected);
            }
            Event::IceConnectionStateChange(state) => {
                tracing::debug!("webrtc: ICE {state:?}");
                match state {
                    // Before `Connected` these are only ICE not having found a pair yet. After
                    // it, Disconnected is an outage and Checking is ICE searching for a new pair
                    // (a handover): the path is down until a pair is back.
                    IceConnectionState::Disconnected | IceConnectionState::Checking => {
                        if self.connected && self.disconnected_since.is_none() {
                            tracing::warn!("webrtc: the ICE path is down");
                            self.disconnected_since = Some(now);
                            self.events.push(TransportEvent::Disconnected);
                        }
                    }
                    IceConnectionState::Connected | IceConnectionState::Completed
                        if self.disconnected_since.take().is_some() =>
                    {
                        tracing::info!("webrtc: the ICE path is back");
                        self.events.push(TransportEvent::Reconnected);
                        // Whatever was written meanwhile went nowhere: start again from a
                        // keyframe.
                        self.idr.push(IdrReason::Reconnected);
                    }
                    _ => {}
                }
            }
            Event::KeyframeRequest(request) => {
                self.idr.push(match request.kind {
                    str0m::media::KeyframeRequestKind::Fir => IdrReason::Fir,
                    _ => IdrReason::Pli,
                });
            }
            Event::ChannelOpen(id, label) => self.on_channel_open(id, label),
            Event::ChannelData(data) => self.on_channel_data(data.id, data.data, data.binary),
            Event::ChannelClose(id) => {
                let label = match self.role(id) {
                    Some(Role::Control) => {
                        self.control_open = false;
                        CONTROL_LABEL
                    }
                    Some(Role::Gamepad) => {
                        self.gamepad_open = false;
                        GAMEPAD_LABEL
                    }
                    Some(Role::Input) => {
                        self.input = None;
                        INPUT_LABEL
                    }
                    // A channel the host refused, or closed itself.
                    None => return,
                };
                tracing::warn!("webrtc: the {label} channel closed");
                self.events.push(TransportEvent::ChannelClosed {
                    label: label.into(),
                });
            }
            Event::Closed => self.closed = true,
            _ => {}
        }
    }

    /// The `input` channel carries keystrokes and clicks: in order, and never dropped.
    fn ordered_and_reliable(&mut self, id: ChannelId) -> bool {
        self.rtc
            .channel(id)
            .and_then(|channel| {
                channel
                    .config()
                    .map(|c| c.ordered && matches!(c.reliability, Reliability::Reliable))
            })
            .unwrap_or(false)
    }

    fn on_channel_open(&mut self, id: ChannelId, label: String) {
        let input_ok = label == INPUT_LABEL
            && self.input.is_none()
            && self.authenticated.load(Ordering::Acquire);
        if input_ok && !self.ordered_and_reliable(id) {
            tracing::warn!("webrtc: the client's input channel is not ordered and reliable");
            self.rtc.direct_api().close_data_channel(id);
            self.events.push(TransportEvent::ChannelRejected(label));
            return;
        }
        match label.as_str() {
            CONTROL_LABEL if id == self.ids.control => {
                self.control_open = true;
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            GAMEPAD_LABEL if id == self.ids.gamepad => {
                self.gamepad_open = true;
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            INPUT_LABEL if input_ok => {
                self.input = Some(id);
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            _ => {
                tracing::warn!("webrtc: refusing the client's {label:?} channel");
                self.rtc.direct_api().close_data_channel(id);
                self.events.push(TransportEvent::ChannelRejected(label));
            }
        }
    }

    fn role(&self, id: ChannelId) -> Option<Role> {
        if id == self.ids.control && self.control_open {
            Some(Role::Control)
        } else if id == self.ids.gamepad && self.gamepad_open {
            Some(Role::Gamepad)
        } else if self.input == Some(id) {
            Some(Role::Input)
        } else {
            None
        }
    }

    fn on_channel_data(&mut self, id: ChannelId, data: Vec<u8>, binary: bool) {
        match self.role(id) {
            Some(Role::Control) => self.events.push(TransportEvent::Control { data, binary }),
            Some(Role::Gamepad) => self.events.push(TransportEvent::Gamepad { data, binary }),
            Some(Role::Input) => {
                // Authentication can end while the channel is open (the session was revoked).
                if self.authenticated.load(Ordering::Acquire) {
                    self.events.push(TransportEvent::Input { data, binary });
                } else {
                    self.input = None;
                    self.rtc.direct_api().close_data_channel(id);
                    self.events
                        .push(TransportEvent::ChannelRejected(INPUT_LABEL.into()));
                }
            }
            None => tracing::debug!("webrtc: data on an unknown channel dropped"),
        }
    }

    /// Packetize one access unit. `Ok(false)` means the peer isn't connected yet, so nothing was
    /// sent (str0m would drop it); the caller already asked for a keyframe on `Connected`.
    pub fn write_video(&mut self, now: Instant, frame: &VideoFrame) -> Result<bool> {
        if !self.is_connected() {
            return Ok(false);
        }
        let data = match self.codec {
            VideoCodec::Av1 => strip_av1_temporal_delimiters(&frame.data)?,
            _ => Cow::Borrowed(frame.data.as_slice()),
        };
        let time = MediaTime::new(
            ticks(frame.capture.saturating_duration_since(self.start), 90_000),
            Frequency::NINETY_KHZ,
        );
        // The playout delay asks the receiver to show the frame as soon as it is whole.
        self.rtc
            .writer(self.ids.video_mid)
            .context("the video media is not negotiated")?
            .playout_delay(MediaTime::ZERO, MediaTime::ZERO)
            .write(
                Pt::new_with_value(VIDEO_PT),
                frame.capture.min(now),
                time,
                data.into_owned(),
            )
            .map_err(|e| anyhow!("video write failed: {e}"))?;
        let packets = frame.data.len().div_ceil(RTP_PAYLOAD_TARGET).max(1);
        let wire = packets * (DATAGRAM_TARGET + network_pacing::overhead(self.local.is_ipv6()));
        self.frame_wire_bytes = self.frame_wire_bytes.max(wire);
        Ok(true)
    }

    /// One Opus packet. Stereo and multistream payloads are not looked at.
    pub fn write_audio(&mut self, now: Instant, packet: &AudioPacket) -> Result<bool> {
        if !self.is_connected() {
            return Ok(false);
        }
        // The audio thread reads whole WASAPI chunks and stamps every packet from one with the
        // same instant, and the device clock drifts against wall time. Receivers place packets by
        // RTP time, so the clock is the thread's own sample position: a packet dropped upstream
        // or a stall that lost no audio leaves it contiguous, and audio Windows lost leaves
        // exactly its length as a gap. Wall time only anchors the first packet, so the NTP
        // pairing starts near the video's.
        let base = *self.audio_base.get_or_insert_with(|| {
            ticks(packet.capture.saturating_duration_since(self.start), 48_000)
                .saturating_sub(packet.position)
        });
        let at = base + packet.position;
        let time = MediaTime::new(at, Frequency::FORTY_EIGHT_KHZ);
        self.rtc
            .writer(self.ids.audio_mid)
            .context("the audio media is not negotiated")?
            .write(
                Pt::new_with_value(AUDIO_PT),
                packet.capture.min(now),
                time,
                packet.data.clone(),
            )
            .map_err(|e| anyhow!("audio write failed: {e}"))?;
        Ok(true)
    }

    /// `Ok(false)` when the channel isn't open (yet) or its buffer is full.
    pub fn send_control(&mut self, data: &[u8], binary: bool) -> Result<bool> {
        self.send_channel(self.ids.control, self.control_open, data, binary)
    }

    pub fn send_gamepad(&mut self, data: &[u8], binary: bool) -> Result<bool> {
        self.send_channel(self.ids.gamepad, self.gamepad_open, data, binary)
    }

    fn send_channel(
        &mut self,
        id: ChannelId,
        open: bool,
        data: &[u8],
        binary: bool,
    ) -> Result<bool> {
        if !open {
            return Ok(false);
        }
        match self.rtc.channel(id) {
            Some(mut channel) => channel
                .write(binary, data)
                .map_err(|e| anyhow!("channel write failed: {e}")),
            None => Ok(false),
        }
    }

    pub fn disconnect(&mut self) {
        self.rtc.disconnect();
    }
}

/// Paces the video packets and passes everything else straight through.
pub struct SendQueue {
    pacing: Pacing,
    stream_kbps: u32,
    frame_period: Duration,
    ipv6: bool,
    /// When the next paced group may go.
    due: Instant,
    /// Wire bytes of the biggest frame noted since the paced queue was last empty.
    frame_bytes: usize,
    immediate: VecDeque<Outgoing>,
    paced: VecDeque<Outgoing>,
    /// Wire bytes waiting in `paced`.
    paced_wire_bytes: usize,
    loss: Loss,
}

/// Which video datagrams a lossy link would eat: a fixed-seed xorshift, so two runs lose the
/// same share, not the same packets.
struct Loss {
    permille: u32,
    state: u64,
}

impl Loss {
    fn drops(&mut self) -> bool {
        if self.permille == 0 {
            return false;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 11) % 1000 < u64::from(self.permille)
    }
}

#[derive(Default)]
pub struct Batch {
    pub packets: Vec<Outgoing>,
    /// Bytes of the paced packets, for [`SendQueue::sent`].
    pub paced_bytes: usize,
    pub paced_packets: usize,
    /// The rate the group was cut for.
    rate_bps: u64,
}

impl SendQueue {
    pub fn new(
        pacing: Pacing,
        stream_kbps: u32,
        frame_period: Duration,
        ipv6: bool,
        now: Instant,
    ) -> Self {
        Self {
            pacing,
            stream_kbps,
            frame_period,
            ipv6,
            due: now,
            frame_bytes: 0,
            immediate: VecDeque::new(),
            paced: VecDeque::new(),
            paced_wire_bytes: 0,
            loss: Loss {
                permille: 0,
                state: 0x9e37_79b9_7f4a_7c15,
            },
        }
    }

    /// Drops this many video datagrams per thousand at the socket.
    pub fn with_loss(mut self, permille: u32) -> Self {
        self.loss.permille = permille;
        self
    }

    /// Whether the simulated link loses this datagram. Only video, as `webrtc_loss_permille`
    /// promises; the datagram is counted as lost, not as sent.
    pub fn lost_in_the_air(&mut self, out: &Outgoing) -> bool {
        out.paced && self.loss.drops()
    }

    /// A frame of this many wire bytes is about to be queued: it sets the rate it needs and the
    /// backlog that is still normal.
    pub fn note_frame(&mut self, wire_bytes: usize) {
        self.frame_bytes = self.frame_bytes.max(wire_bytes);
    }

    pub fn push(&mut self, out: Outgoing) {
        if out.paced && self.pacing != Pacing::Off {
            self.paced_wire_bytes += out.data.len() + network_pacing::overhead(self.ipv6);
            self.paced.push_back(out);
        } else {
            self.immediate.push_back(out);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.immediate.is_empty() && self.paced.is_empty()
    }

    /// When the next batch may go: now for control traffic, the pacer's time for media.
    pub fn due(&self, now: Instant) -> Option<Instant> {
        if !self.immediate.is_empty() {
            Some(now)
        } else if !self.paced.is_empty() {
            Some(self.due)
        } else {
            None
        }
    }

    /// Wire bits per second the pacer works at: `network_pacing::rate_bps` for the route, with
    /// the biggest frame's own demand as the needed rate. A cap (configured, or 4/5 of the
    /// routed link) holds however much a frame needs.
    pub fn rate(&self) -> u64 {
        match self.pacing {
            Pacing::Off => u64::MAX,
            Pacing::Paced {
                configured_kbps,
                link_bps,
                wireless,
            } => {
                let needed =
                    (self.frame_bytes as f64 * 8. / self.frame_period.as_secs_f64()) as u64;
                network_pacing::rate_bps(
                    configured_kbps,
                    self.stream_kbps,
                    link_bps,
                    wireless,
                    needed,
                )
                .max(1)
            }
        }
    }

    /// More paced media waits than the pacer can deliver on time: [`MAX_PACED_BACKLOG`] at the
    /// pacing rate, and never less than two of the biggest frame (a lone keyframe is not a
    /// backlog however long it takes).
    pub fn backlog_exceeded(&self) -> bool {
        if self.pacing == Pacing::Off {
            return false;
        }
        let by_time = self.rate() as f64 / 8. * MAX_PACED_BACKLOG.as_secs_f64();
        let cap = by_time.max(2. * self.frame_bytes as f64);
        self.paced_wire_bytes as f64 > cap
    }

    /// Drop the waiting media. Returns how many packets went.
    pub fn drop_paced(&mut self) -> usize {
        let dropped = self.paced.len();
        self.paced.clear();
        self.paced_wire_bytes = 0;
        self.frame_bytes = 0;
        dropped
    }

    /// Everything unpaced, plus (when due) up to one millisecond of media at the pacing rate and
    /// always at least one datagram.
    pub fn next_batch(&mut self, now: Instant) -> Batch {
        let mut batch = Batch {
            packets: self.immediate.drain(..).collect(),
            ..Batch::default()
        };
        if now >= self.due && !self.paced.is_empty() {
            batch.rate_bps = self.rate();
            let datagram = self.paced.front().map_or(0, |p| p.data.len());
            let limit = network_pacing::batch_payload_limit(
                batch.rate_bps,
                datagram.max(1),
                self.ipv6,
                usize::MAX,
            );
            while let Some(front) = self.paced.front() {
                if batch.paced_packets > 0 && batch.paced_bytes + front.data.len() > limit {
                    break;
                }
                let packet = self.paced.pop_front().expect("front exists");
                self.paced_wire_bytes = self
                    .paced_wire_bytes
                    .saturating_sub(packet.data.len() + network_pacing::overhead(self.ipv6));
                batch.paced_bytes += packet.data.len();
                batch.paced_packets += 1;
                batch.packets.push(packet);
            }
            if self.paced.is_empty() {
                self.frame_bytes = 0;
                self.paced_wire_bytes = 0;
            }
        }
        batch
    }

    /// Called right after the batch's datagrams were written to the socket. The next group is
    /// due one interval after this one was due, so the average rate is exact; a late wake may
    /// send the next groups back to back, but only [`CATCH_UP_CREDIT`] behind schedule.
    pub fn sent(&mut self, completed: Instant, batch: &Batch) {
        if batch.paced_packets == 0 {
            return;
        }
        let wire = batch.paced_bytes + batch.paced_packets * network_pacing::overhead(self.ipv6);
        let interval = Duration::from_secs_f64(wire as f64 * 8. / batch.rate_bps.max(1) as f64);
        let floor = (completed + interval)
            .checked_sub(CATCH_UP_CREDIT)
            .unwrap_or(completed);
        self.due = (self.due + interval).max(floor);
    }
}

#[derive(Default)]
struct Counters {
    video_frames: AtomicU64,
    video_bytes: AtomicU64,
    video_dropped: AtomicU64,
    audio_packets: AtomicU64,
    audio_dropped: AtomicU64,
    datagrams_sent: AtomicU64,
    bytes_sent: AtomicU64,
    send_errors: AtomicU64,
    idr_requests: AtomicU64,
    datagrams_received: AtomicU64,
    /// Received but not queued: the media thread was behind.
    datagrams_dropped: AtomicU64,
    /// Not the peer's, or refused by str0m.
    datagrams_rejected: AtomicU64,
    datagram_errors: AtomicU64,
    recv_errors: AtomicU64,
    /// Paced packets thrown away by the backlog cap.
    paced_dropped: AtomicU64,
    channel_sent: AtomicU64,
    channel_dropped: AtomicU64,
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
}

struct Shared {
    stop: AtomicBool,
    /// Set by every other thread after it queues work, so the media thread's one wait covers
    /// the pacer, str0m's timer and the queues.
    wake: Signal,
    authenticated: Arc<AtomicBool>,
    pending_video: AtomicUsize,
    pending_audio: AtomicUsize,
    pending_datagrams: AtomicUsize,
    /// No frame may go out but a keyframe: nothing has been sent yet, or one was dropped, so the
    /// decoder has no reference.
    need_keyframe: AtomicBool,
    last_idr_ms: AtomicU64,
    epoch: Instant,
    counters: Counters,
    hooks: Hooks,
    options: TransportOptions,
    /// Where the last datagram went: the remote end of the ICE pair str0m selected.
    peer: std::sync::Mutex<Option<SocketAddr>>,
}

impl Shared {
    fn request_idr(&self, reason: IdrReason) {
        self.counters.idr_requests.fetch_add(1, Ordering::Relaxed);
        (self.hooks.idr)(reason);
    }

    fn notify(&self) {
        if let Err(e) = self.wake.set() {
            tracing::error!("webrtc: waking the media thread failed: {e:#}");
        }
    }
}

enum Command {
    Datagram {
        at: Instant,
        from: SocketAddr,
        data: Vec<u8>,
    },
    Video(VideoFrame),
    Audio(AudioPacket),
    Control {
        data: Vec<u8>,
        binary: bool,
    },
    Gamepad {
        data: Vec<u8>,
        binary: bool,
    },
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submit {
    Queued,
    /// Dropped: the queue was full, the session is gone, or the frame needs a reference that was
    /// dropped. The IDR request has been made.
    Dropped,
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

/// The running transport. Dropping it stops the threads.
pub struct MediaHandle {
    tx: Sender<Command>,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    local: SocketAddr,
    audio_layout: Option<OpusLayout>,
}

/// The audio side of a [`MediaHandle`], for the thread that encodes the host's sound. It does not
/// own the transport: closing the handle stops the threads, and a packet sent after that is
/// dropped.
#[derive(Clone)]
pub struct AudioSender {
    tx: Sender<Command>,
    shared: Arc<Shared>,
    layout: OpusLayout,
}

impl AudioSender {
    /// The multistream layout the encoder must use: the one the client answered with.
    pub fn layout(&self) -> &OpusLayout {
        &self.layout
    }

    /// Never blocks; a full queue drops the packet (Opus conceals one).
    pub fn submit(&self, packet: AudioPacket) -> Submit {
        queue_audio(&self.shared, &self.tx, packet)
    }
}

/// The `control` channel of a [`MediaHandle`], for the input worker. Like [`AudioSender`] it does
/// not own the transport: a message sent after the handle closed is counted in `channel_dropped`.
#[derive(Clone)]
pub struct ControlSender {
    tx: Sender<Command>,
    shared: Arc<Shared>,
}

impl ControlSender {
    /// Never blocks. False when the transport is gone.
    pub fn send(&self, data: Vec<u8>, binary: bool) -> bool {
        if self.tx.send(Command::Control { data, binary }).is_err() {
            self.shared
                .counters
                .channel_dropped
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.shared.notify();
        true
    }
}

fn queue_audio(shared: &Shared, tx: &Sender<Command>, packet: AudioPacket) -> Submit {
    if shared.pending_audio.fetch_add(1, Ordering::AcqRel) >= shared.options.audio_queue
        || tx.send(Command::Audio(packet)).is_err()
    {
        shared.pending_audio.fetch_sub(1, Ordering::AcqRel);
        shared
            .counters
            .audio_dropped
            .fetch_add(1, Ordering::Relaxed);
        return Submit::Dropped;
    }
    shared.notify();
    Submit::Queued
}

impl MediaHandle {
    /// Start the media thread for an accepted answer. `local` is the advertised host candidate.
    pub fn spawn(
        socket: UdpSocket,
        local: SocketAddr,
        session: Session<'_>,
        options: TransportOptions,
        hooks: Hooks,
    ) -> Result<Self> {
        let Session {
            rtc,
            ids,
            spec,
            authenticated,
        } = session;
        let epoch = Instant::now();
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            wake: Signal::new().context("creating the media thread's wake event")?,
            authenticated: authenticated.clone(),
            pending_video: AtomicUsize::new(0),
            pending_audio: AtomicUsize::new(0),
            pending_datagrams: AtomicUsize::new(0),
            need_keyframe: AtomicBool::new(true),
            last_idr_ms: AtomicU64::new(0),
            epoch,
            counters: Counters::default(),
            hooks,
            options,
            peer: std::sync::Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel::<Command>();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .context("setting the receive timeout")?;
        let recv_socket = socket.try_clone().context("cloning the media socket")?;
        let engine = Engine::new(rtc, ids, spec, local, epoch, authenticated);
        let queue = SendQueue::new(
            options.pacing,
            spec.video.bitrate_kbps,
            Duration::from_secs_f64(1. / f64::from(spec.video.fps.max(1))),
            local.is_ipv6(),
            Instant::now(),
        )
        .with_loss(options.loss_permille);
        let (shared_rx, tx_rx) = (shared.clone(), tx.clone());
        let shared_media = shared.clone();
        let threads = start_threads(
            &shared,
            || {
                std::thread::Builder::new()
                    .name("webrtc-recv".into())
                    .spawn(move || receive_loop(recv_socket, tx_rx, shared_rx))
            },
            || {
                std::thread::Builder::new()
                    .name("webrtc-media".into())
                    .spawn(move || media_loop(engine, queue, socket, rx, shared_media))
            },
        )?;
        Ok(Self {
            tx,
            shared,
            threads,
            local,
            audio_layout: None,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// The client's address on the selected ICE pair, once media has gone out to it.
    pub fn peer(&self) -> Option<SocketAddr> {
        *self.shared.peer.lock().unwrap()
    }

    /// Whether both threads are still running. A session that ended (or was closed) is not.
    pub fn is_running(&self) -> bool {
        !self.threads.is_empty() && self.threads.iter().all(|t| !t.is_finished())
    }

    /// Hand a command to the media thread and wake it. False when the thread is gone.
    fn queue(&self, command: Command) -> bool {
        if self.tx.send(command).is_err() {
            return false;
        }
        self.shared.notify();
        true
    }

    /// Never blocks. A full queue drops the frame and requests an IDR, and until a keyframe goes
    /// through nothing else is queued: it would reference the dropped frame.
    pub fn submit_video(&self, frame: VideoFrame) -> Submit {
        let shared = &self.shared;
        if shared.need_keyframe.load(Ordering::Acquire) && !frame.keyframe {
            shared
                .counters
                .video_dropped
                .fetch_add(1, Ordering::Relaxed);
            let now_ms = shared.epoch.elapsed().as_millis() as u64;
            let last = shared.last_idr_ms.load(Ordering::Relaxed);
            if now_ms.saturating_sub(last) >= IDR_REQUEST_INTERVAL.as_millis() as u64
                && shared
                    .last_idr_ms
                    .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                shared.request_idr(IdrReason::WaitingForKeyframe);
            }
            return Submit::Dropped;
        }
        if shared.pending_video.fetch_add(1, Ordering::AcqRel) >= shared.options.video_queue {
            shared.pending_video.fetch_sub(1, Ordering::AcqRel);
            return self.drop_video(IdrReason::QueueFull);
        }
        // Cleared before the send: the media thread may drop this very keyframe at once and set
        // the flag again, and that must win over this store.
        if frame.keyframe {
            shared.need_keyframe.store(false, Ordering::Release);
        }
        if !self.queue(Command::Video(frame)) {
            shared.pending_video.fetch_sub(1, Ordering::AcqRel);
            shared.need_keyframe.store(true, Ordering::Release);
            shared
                .counters
                .video_dropped
                .fetch_add(1, Ordering::Relaxed);
            return Submit::Dropped;
        }
        Submit::Queued
    }

    fn drop_video(&self, reason: IdrReason) -> Submit {
        let shared = &self.shared;
        shared
            .counters
            .video_dropped
            .fetch_add(1, Ordering::Relaxed);
        shared.need_keyframe.store(true, Ordering::Release);
        shared
            .last_idr_ms
            .store(shared.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        shared.request_idr(reason);
        Submit::Dropped
    }

    /// Never blocks; a full queue drops the packet (Opus conceals one).
    pub fn submit_audio(&self, packet: AudioPacket) -> Submit {
        queue_audio(&self.shared, &self.tx, packet)
    }

    /// Record the Opus layout the answer agreed to, which [`AudioSender::layout`] hands the audio
    /// thread's encoder.
    pub fn with_audio_layout(mut self, layout: OpusLayout) -> Self {
        self.audio_layout = Some(layout);
        self
    }

    /// The audio end of the transport for the audio thread, which starts before the video worker
    /// takes this handle. `None` until the layout is recorded.
    pub fn audio_sender(&self) -> Option<AudioSender> {
        Some(AudioSender {
            tx: self.tx.clone(),
            shared: self.shared.clone(),
            layout: self.audio_layout.clone()?,
        })
    }

    pub fn send_control(&self, data: Vec<u8>, binary: bool) {
        self.send_channel(Command::Control { data, binary });
    }

    pub fn send_gamepad(&self, data: Vec<u8>, binary: bool) {
        self.send_channel(Command::Gamepad { data, binary });
    }

    fn send_channel(&self, command: Command) {
        if !self.queue(command) {
            self.shared
                .counters
                .channel_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The control-channel end of the transport for the input worker (rumble, mouse-mode), which
    /// does not own the handle.
    pub fn control_sender(&self) -> ControlSender {
        ControlSender {
            tx: self.tx.clone(),
            shared: self.shared.clone(),
        }
    }

    /// Whether the client may open the `input` channel. Set it once the session is authenticated.
    pub fn set_authenticated(&self, authenticated: bool) {
        self.shared
            .authenticated
            .store(authenticated, Ordering::Release);
    }

    pub fn stats(&self) -> TransportStats {
        let c = &self.shared.counters;
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        TransportStats {
            video_frames: get(&c.video_frames),
            video_bytes: get(&c.video_bytes),
            video_dropped: get(&c.video_dropped),
            audio_packets: get(&c.audio_packets),
            audio_dropped: get(&c.audio_dropped),
            datagrams_sent: get(&c.datagrams_sent),
            bytes_sent: get(&c.bytes_sent),
            send_errors: get(&c.send_errors),
            idr_requests: get(&c.idr_requests),
            datagrams_received: get(&c.datagrams_received),
            datagrams_dropped: get(&c.datagrams_dropped),
            datagrams_rejected: get(&c.datagrams_rejected),
            datagram_errors: get(&c.datagram_errors),
            recv_errors: get(&c.recv_errors),
            paced_dropped: get(&c.paced_dropped),
            channel_sent: get(&c.channel_sent),
            channel_dropped: get(&c.channel_dropped),
        }
    }

    /// Stop and join the threads. Called from one of the transport's own hooks (the event hook
    /// runs on the media thread) it leaves that thread to finish by itself: a thread can't join
    /// itself.
    pub fn close(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.tx.send(Command::Stop);
        self.shared.notify();
        let current = std::thread::current().id();
        for thread in self.threads.drain(..) {
            if thread.thread().id() != current {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        self.close();
    }
}

/// Start the receive and media threads. If the media thread cannot start, the receive thread is
/// stopped and joined so the socket (and its port) is released before the error returns.
fn start_threads(
    shared: &Shared,
    receive: impl FnOnce() -> std::io::Result<JoinHandle<()>>,
    media: impl FnOnce() -> std::io::Result<JoinHandle<()>>,
) -> Result<Vec<JoinHandle<()>>> {
    let receive = receive().context("starting the receive thread")?;
    match media() {
        Ok(media) => Ok(vec![receive, media]),
        Err(e) => {
            shared.stop.store(true, Ordering::Release);
            let _ = receive.join();
            Err(anyhow::Error::new(e).context("starting the media thread"))
        }
    }
}

/// Consecutive receive errors tolerated before the receive thread backs off: a lone error costs
/// one datagram and no latency, a persistent one must not spin the thread.
const RECV_ERRORS_BEFORE_BACKOFF: u32 = 8;
const RECV_ERROR_BACKOFF: Duration = Duration::from_millis(5);

/// Count one more consecutive receive error; the pause to take, if any.
fn recv_error_backoff(consecutive: &mut u32) -> Option<Duration> {
    *consecutive = consecutive.saturating_add(1);
    (*consecutive >= RECV_ERRORS_BEFORE_BACKOFF).then_some(RECV_ERROR_BACKOFF)
}

fn receive_loop(socket: UdpSocket, tx: Sender<Command>, shared: Arc<Shared>) {
    let mut buf = vec![0u8; RECV_DATAGRAM_BYTES];
    let mut consecutive_errors = 0u32;
    while !shared.stop.load(Ordering::Acquire) {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                consecutive_errors = 0;
                let counters = &shared.counters;
                counters.datagrams_received.fetch_add(1, Ordering::Relaxed);
                if shared.pending_datagrams.fetch_add(1, Ordering::AcqRel)
                    >= shared.options.datagram_queue
                {
                    shared.pending_datagrams.fetch_sub(1, Ordering::AcqRel);
                    if counters
                        .datagrams_dropped
                        .fetch_add(1, Ordering::Relaxed)
                        .is_multiple_of(1000)
                    {
                        tracing::warn!(
                            "webrtc: the media thread is behind, dropping received datagrams"
                        );
                    }
                    continue;
                }
                let command = Command::Datagram {
                    at: Instant::now(),
                    from,
                    data: buf[..len].to_vec(),
                };
                if tx.send(command).is_err() {
                    return;
                }
                shared.notify();
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                consecutive_errors = 0
            }
            // Windows reports an ICMP port-unreachable for an earlier send as a receive error.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            // Anything else costs the datagram, not the session: whoever can reach this port
            // must not be able to end it.
            Err(e) => {
                let n = shared.counters.recv_errors.fetch_add(1, Ordering::Relaxed);
                if n.is_multiple_of(100) {
                    tracing::warn!(
                        "webrtc: media socket receive failed ({} so far): {e}",
                        n + 1
                    );
                }
                if let Some(pause) = recv_error_backoff(&mut consecutive_errors) {
                    std::thread::sleep(pause);
                }
            }
        }
    }
}

fn media_loop(
    mut engine: Engine,
    mut queue: SendQueue,
    socket: UdpSocket,
    rx: mpsc::Receiver<Command>,
    shared: Arc<Shared>,
) {
    let result = (|| -> Result<()> {
        // The high-resolution waitable timer the host's other senders pace with; it belongs to
        // this thread.
        let timer = Timer::new().context("creating the pacing timer")?;
        loop {
            if shared.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            // Reset before draining: whatever is queued after the drain sets it again, so the
            // wait below returns at once instead of sleeping on it.
            shared.wake.reset()?;
            loop {
                match rx.try_recv() {
                    Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                    Ok(command) => handle_command(&mut engine, &shared, command),
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let timeout = engine.advance(Instant::now())?;
            // The frame's size first: it sets the pace its own packets are sent at.
            let frame = engine.take_frame_wire_bytes();
            if frame > 0 {
                queue.note_frame(frame);
            }
            for out in engine.take_outgoing() {
                queue.push(out);
            }
            enforce_backlog(&mut queue, &shared);
            dispatch(&mut engine, &shared);
            if engine.is_closed() {
                return Ok(());
            }
            send_due(&socket, &mut queue, &shared);
            let (rejected, failed) = engine.dropped_datagrams();
            shared
                .counters
                .datagrams_rejected
                .store(rejected, Ordering::Relaxed);
            shared
                .counters
                .datagram_errors
                .store(failed, Ordering::Relaxed);
            match queue.due(Instant::now()) {
                Some(due) if due < timeout => {
                    timer.until_paced_or_signal(due, &shared.wake)?;
                }
                _ => {
                    timer.until_or_signal(timeout, &shared.wake)?;
                }
            }
        }
    })();
    if let Err(e) = result {
        tracing::error!("webrtc: media thread stopped: {e:#}");
    }
    engine.disconnect();
    (shared.hooks.event)(TransportEvent::Closed);
}

/// The pacer fell further behind than it can ever catch up: what it holds is stale, so drop it
/// and start over from a keyframe instead of delivering late.
fn enforce_backlog(queue: &mut SendQueue, shared: &Shared) {
    if !queue.backlog_exceeded() {
        return;
    }
    let dropped = queue.drop_paced() as u64;
    let total = shared
        .counters
        .paced_dropped
        .fetch_add(dropped, Ordering::Relaxed)
        + dropped;
    tracing::warn!(
        "webrtc: the pacer fell behind, dropped {dropped} packets ({total} so far), asking for a keyframe"
    );
    shared.need_keyframe.store(true, Ordering::Release);
    shared.request_idr(IdrReason::PacerBacklog);
}

fn handle_command(engine: &mut Engine, shared: &Shared, command: Command) {
    let now = Instant::now();
    let counters = &shared.counters;
    match command {
        Command::Datagram { at, from, data } => {
            shared.pending_datagrams.fetch_sub(1, Ordering::AcqRel);
            // The outcome is counted from the engine's own tallies.
            engine.handle_datagram(at.min(now), from, &data);
        }
        Command::Video(frame) => {
            shared.pending_video.fetch_sub(1, Ordering::AcqRel);
            if shared.need_keyframe.load(Ordering::Acquire) && !frame.keyframe {
                // Queued before a drop: it references a frame that never left.
                counters.video_dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            match engine.write_video(now, &frame) {
                Ok(true) => {
                    counters.video_frames.fetch_add(1, Ordering::Relaxed);
                    counters
                        .video_bytes
                        .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                    if frame.keyframe {
                        shared.need_keyframe.store(false, Ordering::Release);
                    }
                }
                Ok(false) => {
                    // Not connected, or the path is down: this frame never left, so the next
                    // one needs a keyframe (`Connected` and `Reconnected` ask for it).
                    counters.video_dropped.fetch_add(1, Ordering::Relaxed);
                    shared.need_keyframe.store(true, Ordering::Release);
                }
                Err(e) => {
                    tracing::error!("webrtc: {e:#}");
                    counters.video_dropped.fetch_add(1, Ordering::Relaxed);
                    shared.need_keyframe.store(true, Ordering::Release);
                    shared.request_idr(IdrReason::WriteFailed);
                }
            }
        }
        Command::Audio(packet) => {
            shared.pending_audio.fetch_sub(1, Ordering::AcqRel);
            match engine.write_audio(now, &packet) {
                Ok(true) => {
                    counters.audio_packets.fetch_add(1, Ordering::Relaxed);
                }
                Ok(false) => {
                    counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::error!("webrtc: {e:#}");
                    counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Command::Control { data, binary } => {
            let sent = engine.send_control(&data, binary);
            note_channel_send(shared, "control", sent);
        }
        Command::Gamepad { data, binary } => {
            let sent = engine.send_gamepad(&data, binary);
            note_channel_send(shared, "gamepad", sent);
        }
        Command::Stop => {}
    }
}

/// A message for the client that couldn't go: the channel isn't open, its buffer is full, or
/// the write failed. Counted, and logged for the first and every hundredth.
fn note_channel_send(shared: &Shared, channel: &str, sent: Result<bool>) {
    let counters = &shared.counters;
    let why = match sent {
        Ok(true) => {
            counters.channel_sent.fetch_add(1, Ordering::Relaxed);
            return;
        }
        Ok(false) => "the channel is not open or its buffer is full".to_string(),
        Err(e) => format!("{e:#}"),
    };
    let n = counters.channel_dropped.fetch_add(1, Ordering::Relaxed);
    if n.is_multiple_of(100) {
        tracing::warn!(
            "webrtc: {channel} message not sent ({} so far): {why}",
            n + 1
        );
    }
}

fn dispatch(engine: &mut Engine, shared: &Shared) {
    for reason in engine.take_idr_requests() {
        shared.request_idr(reason);
    }
    for event in engine.take_events() {
        (shared.hooks.event)(event);
    }
}

fn send_due(socket: &UdpSocket, queue: &mut SendQueue, shared: &Shared) {
    let mut now = Instant::now();
    // The pacing timer wakes up to its learned lead early; that is on schedule.
    while queue.due(now).is_some_and(|due| due <= now + EARLY_SEND) {
        let batch = queue.next_batch(now + EARLY_SEND);
        if batch.packets.is_empty() {
            break;
        }
        if let Some(last) = batch.packets.last() {
            let mut peer = shared.peer.lock().unwrap();
            if *peer != Some(last.destination) {
                *peer = Some(last.destination);
            }
        }
        for out in &batch.packets {
            if queue.lost_in_the_air(out) {
                continue;
            }
            match socket.send_to(&out.data, out.destination) {
                Ok(_) => {
                    shared
                        .counters
                        .datagrams_sent
                        .fetch_add(1, Ordering::Relaxed);
                    shared
                        .counters
                        .bytes_sent
                        .fetch_add(out.data.len() as u64, Ordering::Relaxed);
                }
                Err(e) => {
                    // Loud, but not fatal: the peer may be between networks.
                    if shared
                        .counters
                        .send_errors
                        .fetch_add(1, Ordering::Relaxed)
                        .is_multiple_of(100)
                    {
                        tracing::warn!("webrtc: sending to {} failed: {e}", out.destination);
                    }
                }
            }
        }
        now = Instant::now();
        queue.sent(now, &batch);
    }
}

#[cfg(test)]
mod tests {
    use super::super::sdp::testing::{answer, answer_for, client, host_offer, spec, surround_spec};
    use super::super::sdp::{VideoCodec, accept_answer};
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use str0m::media::{KeyframeRequestKind, Mid};
    use str0m::rtp::RawPacket;

    const HOST: &str = "192.168.1.10:8001";
    const CLIENT: &str = "192.168.1.20:50000";

    #[derive(Clone, Debug)]
    struct Rtp {
        pt: u8,
        marker: bool,
        timestamp: u32,
        seq: u16,
        playout: Option<(MediaTime, MediaTime)>,
    }

    #[derive(Debug)]
    struct Received {
        pt: u8,
        data: Vec<u8>,
        time: MediaTime,
    }

    /// A host Engine against a plain str0m client, joined by an in-memory link and a virtual clock.
    struct Link {
        host: Engine,
        client: Rtc,
        client_video: Option<Mid>,
        now: Instant,
        start: Instant,
        authenticated: Arc<AtomicBool>,
        rtp: Vec<Rtp>,
        media: Vec<Received>,
        host_events: Vec<TransportEvent>,
        idr: Vec<IdrReason>,
        channel_data: Vec<(String, Vec<u8>)>,
        labels: HashMap<ChannelId, String>,
        closed_by_host: Vec<String>,
        /// Drop the n-th host media datagram (counting from zero).
        drop_rtp: Option<usize>,
        rtp_seen: usize,
        /// Header extension ids on the wire, from the unencrypted part of every media datagram.
        wire_ext_ids: std::collections::BTreeSet<u8>,
        /// Drop every datagram in both directions (the path is down).
        partition: bool,
        /// The host's ICE username fragment, for forging STUN.
        host_ufrag: String,
    }

    /// The ids of the one-byte header extensions in an RTP packet (RFC 8285).
    fn extension_ids(packet: &[u8]) -> Vec<u8> {
        let mut ids = Vec::new();
        if packet[0] & 0x10 == 0 {
            return ids;
        }
        let at = 12 + 4 * usize::from(packet[0] & 0x0f);
        if packet[at..at + 2] != [0xbe, 0xde] {
            return ids;
        }
        let words = usize::from(u16::from_be_bytes([packet[at + 2], packet[at + 3]]));
        let block = &packet[at + 4..at + 4 + 4 * words];
        let mut i = 0;
        while i < block.len() {
            if block[i] == 0 {
                i += 1;
                continue;
            }
            ids.push(block[i] >> 4);
            i += 1 + usize::from(block[i] & 0x0f) + 1;
        }
        ids
    }

    /// The client's answer for a surround request: str0m's own client only knows stereo Opus, so
    /// it answers that and its Opus line is munged to multiopus the way Chromium would send it.
    fn multiopus_answer(spec: &OfferSpec, offer: &str, client: &mut Rtc) -> String {
        answer_for(client, offer, spec)
    }

    impl Link {
        fn new(spec: OfferSpec) -> Self {
            let host_addr: SocketAddr = HOST.parse().unwrap();
            let (mut rtc, offer) = host_offer(&spec, host_addr);
            let host_ufrag = offer
                .sdp
                .lines()
                .find_map(|l| l.strip_prefix("a=ice-ufrag:"))
                .expect("the offer has an ice-ufrag")
                .trim()
                .to_string();
            let mut client = client(&spec, CLIENT.parse().unwrap());
            let answer_text = multiopus_answer(&spec, &offer.sdp, &mut client);
            let (ids, _) = accept_answer(&mut rtc, offer, &answer_text, &spec).unwrap();
            let now = Instant::now();
            let authenticated = Arc::new(AtomicBool::new(false));
            let host = Engine::new(rtc, ids, &spec, host_addr, now, authenticated.clone());
            Self {
                host,
                client,
                client_video: None,
                now,
                start: now,
                authenticated,
                rtp: Vec::new(),
                media: Vec::new(),
                host_events: Vec::new(),
                idr: Vec::new(),
                channel_data: Vec::new(),
                labels: HashMap::new(),
                closed_by_host: Vec::new(),
                drop_rtp: None,
                rtp_seen: 0,
                wire_ext_ids: Default::default(),
                partition: false,
                host_ufrag,
            }
        }

        fn connected(spec: OfferSpec) -> Self {
            let mut link = Self::new(spec);
            for _ in 0..3000 {
                link.step(Duration::from_millis(1));
                let open = |l: &str| {
                    link.host_events
                        .contains(&TransportEvent::ChannelOpen(l.into()))
                };
                if link.host.is_connected() && open("control") && open("gamepad") {
                    return link;
                }
            }
            panic!("never connected: {:?}", link.host_events);
        }

        /// Move the clock `dt` and carry datagrams until the link is quiet.
        fn step(&mut self, dt: Duration) {
            self.now += dt;
            let (host_addr, client_addr): (SocketAddr, SocketAddr) =
                (HOST.parse().unwrap(), CLIENT.parse().unwrap());
            for _ in 0..200 {
                let mut moved = false;
                self.host.advance(self.now).unwrap();
                self.client.handle_input(Input::Timeout(self.now)).unwrap();
                self.collect_host();
                for out in self.host.take_outgoing() {
                    moved = true;
                    if out.paced {
                        self.wire_ext_ids.extend(extension_ids(&out.data));
                        let n = self.rtp_seen;
                        self.rtp_seen += 1;
                        if self.drop_rtp == Some(n) {
                            continue;
                        }
                    }
                    if self.partition {
                        continue;
                    }
                    let recv =
                        Receive::new(Protocol::Udp, host_addr, client_addr, &out.data).unwrap();
                    self.client
                        .handle_input(Input::Receive(self.now, recv))
                        .unwrap();
                }
                loop {
                    match self.client.poll_output().unwrap() {
                        Output::Timeout(_) => break,
                        Output::Transmit(t) => {
                            moved = true;
                            let data: Vec<u8> = t.contents.into();
                            if !self.partition {
                                let fate = self.host.handle_datagram(self.now, client_addr, &data);
                                assert_eq!(fate, Datagram::Handled, "the real client's datagram");
                            }
                        }
                        Output::Event(e) => self.on_client_event(e),
                    }
                }
                self.collect_host();
                if !moved {
                    break;
                }
            }
        }

        fn run(&mut self, total: Duration) {
            for _ in 0..total.as_millis() {
                self.step(Duration::from_millis(1));
            }
        }

        fn collect_host(&mut self) {
            self.host_events.extend(self.host.take_events());
            self.idr.extend(self.host.take_idr_requests());
        }

        fn on_client_event(&mut self, e: Event) {
            match e {
                Event::MediaAdded(added) if added.kind.is_video() => {
                    self.client_video = Some(added.mid)
                }
                Event::MediaData(d) => self.media.push(Received {
                    pt: *d.pt,
                    data: d.data.to_vec(),
                    time: d.time,
                }),
                Event::RawPacket(raw) => {
                    if let RawPacket::RtpRx(h, _) = *raw {
                        self.rtp.push(Rtp {
                            pt: *h.payload_type,
                            marker: h.marker,
                            timestamp: h.timestamp,
                            seq: h.sequence_number,
                            playout: h.ext_vals.play_delay_min.zip(h.ext_vals.play_delay_max),
                        });
                    }
                }
                Event::ChannelOpen(id, label) => {
                    self.labels.insert(id, label);
                }
                Event::ChannelData(d) => {
                    let label = self.labels.get(&d.id).cloned().unwrap_or_default();
                    self.channel_data.push((label, d.data));
                }
                Event::ChannelClose(id) => {
                    if let Some(label) = self.labels.get(&id) {
                        self.closed_by_host.push(label.clone());
                    }
                }
                _ => {}
            }
        }

        fn at(&self, ms: u64) -> Instant {
            self.start + Duration::from_millis(ms)
        }

        fn video(&mut self, data: Vec<u8>, at_ms: u64, keyframe: bool) {
            let frame = VideoFrame {
                data,
                capture: self.at(at_ms),
                keyframe,
            };
            assert!(
                self.host.write_video(self.now, &frame).unwrap(),
                "not connected"
            );
        }

        fn audio(&mut self, data: Vec<u8>, at_ms: u64, position: u64) {
            let packet = AudioPacket {
                data,
                capture: self.at(at_ms),
                position,
            };
            assert!(
                self.host.write_audio(self.now, &packet).unwrap(),
                "not connected"
            );
        }

        fn client_channel(&self, label: &str) -> ChannelId {
            *self
                .labels
                .iter()
                .find(|(_, l)| l.as_str() == label)
                .unwrap_or_else(|| panic!("no {label} channel"))
                .0
        }

        fn client_writes(&mut self, label: &str, data: &[u8]) {
            let id = self.client_channel(label);
            self.client_writes_to(id, data);
        }

        fn client_writes_to(&mut self, id: ChannelId, data: &[u8]) {
            assert!(self.client.channel(id).unwrap().write(true, data).unwrap());
        }

        fn client_opens(&mut self, label: &str) -> ChannelId {
            let mut api = self.client.sdp_api();
            let id = api.add_channel(label.into());
            assert!(
                api.apply().is_none(),
                "opening a channel should not renegotiate"
            );
            id
        }
    }

    /// Bytes that can't contain a start code or a valid-looking OBU boundary by accident.
    fn filler(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8 % 200).wrapping_add(seed).max(1) | 0x01)
            .collect()
    }

    fn annex_b(nals: &[Vec<u8>]) -> Vec<u8> {
        nals.iter()
            .flat_map(|n| [&[0, 0, 0, 1][..], n.as_slice()].concat())
            .collect()
    }

    fn h264_frame(keyframe: bool, size: usize) -> Vec<u8> {
        if keyframe {
            annex_b(&[
                [&[0x67u8][..], &filler(20, 3)].concat(),
                [&[0x68u8][..], &filler(6, 5)].concat(),
                [&[0x65u8][..], &filler(size, 7)].concat(),
            ])
        } else {
            annex_b(&[[&[0x41u8][..], &filler(size, 9)].concat()])
        }
    }

    fn hevc_frame(keyframe: bool, size: usize) -> Vec<u8> {
        if keyframe {
            annex_b(&[
                [&[0x40u8, 0x01][..], &filler(20, 3)].concat(),
                [&[0x42u8, 0x01][..], &filler(30, 4)].concat(),
                [&[0x44u8, 0x01][..], &filler(6, 5)].concat(),
                [&[0x26u8, 0x01][..], &filler(size, 7)].concat(),
            ])
        } else {
            annex_b(&[[&[0x02u8, 0x01][..], &filler(size, 9)].concat()])
        }
    }

    fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![(kind << 3) | 0x02];
        let mut size = payload.len();
        loop {
            let byte = (size & 0x7f) as u8;
            size >>= 7;
            if size == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.extend_from_slice(payload);
        out
    }

    const TD: [u8; 2] = [0x12, 0x00];

    /// A temporal delimiter, then (for a keyframe) a sequence header, then the frame OBU.
    fn av1_frame(keyframe: bool, size: usize) -> Vec<u8> {
        let mut out = TD.to_vec();
        if keyframe {
            out.extend(obu(1, &filler(12, 3)));
        }
        out.extend(obu(6, &filler(size, 9)));
        out
    }

    fn without_td(frame: &[u8]) -> Vec<u8> {
        frame[TD.len()..].to_vec()
    }

    fn diff(a: &[u8], b: &[u8]) -> String {
        match a.iter().zip(b).position(|(x, y)| x != y) {
            Some(i) => format!(
                "first difference at byte {i} (lengths {} and {})",
                a.len(),
                b.len()
            ),
            None if a.len() != b.len() => {
                format!("same prefix, lengths {} and {}", a.len(), b.len())
            }
            None => "identical".into(),
        }
    }

    #[test]
    fn video_arrives_byte_identical_for_every_codec() {
        type Frame = fn(bool, usize) -> Vec<u8>;
        let cases: [(VideoCodec, Frame, bool); 4] = [
            (VideoCodec::H264, h264_frame, false),
            (VideoCodec::Hevc, hevc_frame, false),
            (VideoCodec::Hevc, hevc_frame, true),
            (VideoCodec::Av1, av1_frame, false),
        ];
        for (codec, make, hdr) in cases {
            let mut link = Link::connected(spec(codec, hdr));
            let sent = [make(true, 30_000), make(false, 4_000), make(false, 900)];
            for (i, frame) in sent.iter().enumerate() {
                link.video(frame.clone(), 16 * i as u64, i == 0);
                link.step(Duration::from_millis(16));
            }
            link.run(Duration::from_millis(50));
            let expected: Vec<Vec<u8>> = sent
                .iter()
                .map(|f| {
                    if codec == VideoCodec::Av1 {
                        without_td(f)
                    } else {
                        f.clone()
                    }
                })
                .collect();
            let video: Vec<&Received> = link.media.iter().filter(|m| m.pt == 96).collect();
            assert_eq!(video.len(), expected.len(), "{codec:?}: frames received");
            for (i, (got, want)) in video.iter().zip(&expected).enumerate() {
                assert!(
                    got.data == *want,
                    "{codec:?} frame {i}: {}",
                    diff(&got.data, want)
                );
            }
        }
    }

    #[test]
    fn marker_bit_is_on_the_last_packet_of_each_frame_only() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.video(h264_frame(true, 30_000), 0, true);
        link.step(Duration::from_millis(16));
        link.video(h264_frame(false, 5_000), 16, false);
        link.run(Duration::from_millis(50));
        let video: Vec<&Rtp> = link.rtp.iter().filter(|p| p.pt == 96).collect();
        let mut frames: Vec<Vec<&Rtp>> = Vec::new();
        for p in video {
            match frames.last_mut() {
                Some(f) if f[0].timestamp == p.timestamp => f.push(p),
                _ => frames.push(vec![p]),
            }
        }
        assert_eq!(frames.len(), 2);
        assert!(
            frames[0].len() > 20,
            "a 30 KB keyframe is many packets: {}",
            frames[0].len()
        );
        for f in &frames {
            let (last, rest) = f.split_last().unwrap();
            assert!(last.marker, "the last packet of a frame carries the marker");
            assert!(rest.iter().all(|p| !p.marker), "no other packet does");
            assert!(
                f.windows(2).all(|w| w[1].seq == w[0].seq.wrapping_add(1)),
                "sequence numbers are gapless"
            );
        }
    }

    #[test]
    fn rtp_timestamps_follow_the_capture_instant() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        // Captured 0, 7 and 24 ms after the session start, but all handed over in one go.
        for (ms, key) in [(0, true), (7, false), (24, false)] {
            link.video(h264_frame(key, 2_000), ms, key);
        }
        link.run(Duration::from_millis(50));
        let mut stamps: Vec<u32> = Vec::new();
        for p in link.rtp.iter().filter(|p| p.pt == 96) {
            if stamps.last() != Some(&p.timestamp) {
                stamps.push(p.timestamp);
            }
        }
        assert_eq!(stamps.len(), 3);
        assert_eq!(
            stamps[1].wrapping_sub(stamps[0]),
            7 * 90,
            "7 ms is 630 ticks of 90 kHz"
        );
        assert_eq!(stamps[2].wrapping_sub(stamps[1]), 17 * 90);
        let times: Vec<u64> = link
            .media
            .iter()
            .filter(|m| m.pt == 96)
            .map(|m| m.time.numer())
            .collect();
        assert_eq!(times.len(), 3);
        assert_eq!(times[2] - times[0], 24 * 90);
    }

    #[test]
    fn every_video_packet_asks_for_no_playout_delay() {
        let mut link = Link::connected(spec(VideoCodec::Hevc, true));
        link.video(hevc_frame(true, 10_000), 0, true);
        link.run(Duration::from_millis(30));
        let video: Vec<&Rtp> = link.rtp.iter().filter(|p| p.pt == 96).collect();
        assert!(!video.is_empty());
        for p in video {
            let (min, max) = p
                .playout
                .expect("the playout-delay extension is on every packet");
            assert!(min.is_zero() && max.is_zero());
        }
    }

    #[test]
    fn hdr_colour_space_is_never_sent() {
        let mut link = Link::connected(spec(VideoCodec::Hevc, true));
        link.video(hevc_frame(true, 10_000), 0, true);
        link.run(Duration::from_millis(30));
        // Offered (the SDP test) but str0m has no serializer for it, and the playout delay and mid
        // extensions are there, so the block parsing is real.
        assert!(
            link.wire_ext_ids.contains(&6) && link.wire_ext_ids.contains(&4),
            "{:?}",
            link.wire_ext_ids
        );
        assert!(!link.wire_ext_ids.contains(&9), "{:?}", link.wire_ext_ids);
    }

    #[test]
    fn nothing_is_written_before_the_peer_is_connected() {
        let mut link = Link::new(spec(VideoCodec::H264, false));
        let frame = VideoFrame {
            data: h264_frame(true, 1_000),
            capture: link.at(0),
            keyframe: true,
        };
        assert!(!link.host.write_video(link.now, &frame).unwrap());
        let packet = AudioPacket {
            data: vec![1, 2, 3],
            capture: link.at(0),
            position: 0,
        };
        assert!(!link.host.write_audio(link.now, &packet).unwrap());
    }

    #[test]
    fn connecting_asks_for_a_keyframe() {
        let link = Link::connected(spec(VideoCodec::H264, false));
        assert!(link.idr.contains(&IdrReason::Connected), "{:?}", link.idr);
    }

    #[test]
    fn pli_and_fir_reach_the_idr_callback() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.video(h264_frame(true, 3_000), 0, true);
        link.run(Duration::from_millis(30));
        link.idr.clear();
        let mid = link.client_video.expect("video negotiated");
        for kind in [KeyframeRequestKind::Pli, KeyframeRequestKind::Fir] {
            link.client
                .direct_api()
                .stream_rx_by_mid(mid, None)
                .expect("receiving video")
                .request_keyframe(kind);
            link.run(Duration::from_millis(30));
        }
        assert!(link.idr.contains(&IdrReason::Pli), "{:?}", link.idr);
        assert!(link.idr.contains(&IdrReason::Fir), "{:?}", link.idr);
    }

    #[test]
    fn opus_packets_pass_through_untouched() {
        for spec in [
            spec(VideoCodec::H264, false),
            surround_spec(6),
            surround_spec(8),
        ] {
            let channels = spec.audio.channels;
            let mut link = Link::connected(spec);
            // Not valid Opus: the transport must not look inside.
            let packets: Vec<Vec<u8>> = [120, 300, 640]
                .iter()
                .enumerate()
                .map(|(i, n)| filler(*n, 20 + i as u8))
                .collect();
            for (i, p) in packets.iter().enumerate() {
                link.audio(p.clone(), 10 * i as u64, 480 * i as u64);
                link.step(Duration::from_millis(10));
            }
            link.run(Duration::from_millis(40));
            let got: Vec<&Vec<u8>> = link
                .media
                .iter()
                .filter(|m| m.pt == 111)
                .map(|m| &m.data)
                .collect();
            assert_eq!(got.len(), packets.len(), "{channels} channels");
            for (g, p) in got.iter().zip(&packets) {
                assert!(*g == p, "{channels} channels: {}", diff(g, p));
            }
        }
    }

    /// The RTP times of the Opus packets the client has received so far.
    fn audio_times(link: &Link) -> Vec<u64> {
        link.media
            .iter()
            .filter(|m| m.pt == 111)
            .map(|m| m.time.numer())
            .collect()
    }

    #[test]
    fn audio_packets_stamped_with_one_capture_instant_still_advance_the_rtp_clock() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        // 5 ms CELT packets (240 samples) read in pairs, as the audio thread does from 10 ms WASAPI chunks.
        let packet = |n: u8| vec![0xE8, n, n];
        for i in 0..8u64 {
            link.audio(packet(i as u8), 10 * (i / 2), 240 * i);
            link.step(Duration::from_millis(5));
        }
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 8);
        for pair in times.windows(2) {
            assert_eq!(pair[1] - pair[0], 240, "{times:?}");
        }
    }

    #[test]
    fn a_late_wake_that_drops_nothing_delivers_its_packets_contiguously() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let packet = |n: u8| vec![0xE8, n, n];
        link.audio(packet(0), 0, 0);
        link.step(Duration::from_millis(5));
        // The thread wakes 25 ms late, under the capture's 30 ms trim limit, and reads the five
        // packets that built up at once: one capture instant, five consecutive positions.
        for i in 1..=5u64 {
            link.audio(packet(i as u8), 30, 240 * i);
        }
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 6);
        for pair in times.windows(2) {
            assert_eq!(pair[1] - pair[0], 240, "{times:?}");
        }
    }

    #[test]
    fn a_late_wake_that_trimmed_its_backlog_is_a_gap_as_long_as_the_trim() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let packet = |n: u8| vec![0xE8, n, n];
        link.audio(packet(0), 0, 0);
        link.step(Duration::from_millis(5));
        // 100 ms late: the capture trims 90 ms (4320 samples) of the 100 ms waiting and keeps two
        // packets. The audio thread adds the trim to the position, so the receiver conceals it,
        // and the RTP time is back in step with the wall (105 ms in, 5040 ticks).
        link.audio(packet(1), 105, 240 + 4320);
        link.audio(packet(2), 105, 240 + 4320 + 240);
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 3);
        assert_eq!(times[1] - times[0], 240 + 4320, "{times:?}");
        assert_eq!(times[2] - times[1], 240, "{times:?}");
    }

    #[test]
    fn lost_audio_is_a_gap_of_exactly_its_length_at_any_size() {
        // Around the old 40 ms wall-time threshold: every gap, long or short, is what was lost.
        for lost in [0u64, 1, 1919, 1920, 1921, 9600] {
            let mut link = Link::connected(spec(VideoCodec::H264, false));
            let packet = |n: u8| vec![0xE8, n, n];
            link.audio(packet(0), 0, 0);
            link.step(Duration::from_millis(5));
            link.audio(packet(1), 5, 240 + lost);
            link.run(Duration::from_millis(40));
            let times = audio_times(&link);
            assert_eq!(times.len(), 2, "{lost}");
            assert_eq!(times[1] - times[0], 240 + lost, "{lost}: {times:?}");
        }
    }

    #[test]
    fn a_packet_dropped_upstream_still_advances_the_rtp_clock() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let packet = |n: u8| vec![0xE8, n, n];
        // The queue was full, or the peer not yet connected, for the packets at 240 and 480:
        // the audio thread numbered them anyway.
        link.audio(packet(0), 0, 0);
        link.step(Duration::from_millis(5));
        link.audio(packet(3), 20, 720);
        link.step(Duration::from_millis(5));
        link.audio(packet(4), 25, 960);
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 3);
        assert_eq!(times[1] - times[0], 720, "{times:?}");
        assert_eq!(times[2] - times[1], 240, "{times:?}");
    }

    #[test]
    fn packets_the_host_cannot_measure_do_not_repeat_a_timestamp() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        // A one-byte code 3 packet has no frame count: nothing in it says how long it is.
        for i in 0..4u64 {
            link.audio(vec![0xEB], 5 * i, 240 * i);
            link.step(Duration::from_millis(5));
        }
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len(), 4);
        for pair in times.windows(2) {
            assert_eq!(pair[1] - pair[0], 240, "{times:?}");
        }
    }

    #[test]
    fn a_slow_device_clock_over_an_hour_never_jumps_the_rtp_clock() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        // The device delivers 48 kHz samples 0.05% slower than wall time (1.8 s an hour), with
        // one packet sampled every 3 s of audio, ten times what the old wall-time rule
        // tolerated before resyncing. RTP time must differ by exactly the sample position.
        let step = 144_000u64;
        let packets = 1200u64;
        for i in 0..packets {
            let position = i * step;
            let wall_ms = position / 48 * 10_005 / 10_000;
            link.audio(vec![0xE8, 1, 2], wall_ms, position);
            for _ in 0..60 {
                link.step(Duration::from_millis(50));
            }
        }
        link.run(Duration::from_millis(40));
        let times = audio_times(&link);
        assert_eq!(times.len() as u64, packets);
        for pair in times.windows(2) {
            assert_eq!(pair[1] - pair[0], step, "{pair:?}");
        }
    }

    #[test]
    fn control_and_gamepad_channels_carry_messages_both_ways() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        assert!(
            link.host
                .send_control(b"{\"type\":\"hello\"}", false)
                .unwrap()
        );
        assert!(link.host.send_gamepad(&[1, 2, 3, 4], true).unwrap());
        link.run(Duration::from_millis(30));
        assert!(
            link.channel_data
                .contains(&("control".into(), b"{\"type\":\"hello\"}".to_vec()))
        );
        assert!(
            link.channel_data
                .contains(&("gamepad".into(), vec![1, 2, 3, 4]))
        );

        link.client_writes("control", b"{\"type\":\"idr\"}");
        link.client_writes("gamepad", &[9, 8, 7]);
        link.run(Duration::from_millis(30));
        assert!(link.host_events.contains(&TransportEvent::Control {
            data: b"{\"type\":\"idr\"}".to_vec(),
            binary: true
        }));
        assert!(link.host_events.contains(&TransportEvent::Gamepad {
            data: vec![9, 8, 7],
            binary: true
        }));
    }

    #[test]
    fn the_input_channel_is_accepted_only_when_authenticated() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.client_opens("input");
        link.run(Duration::from_millis(60));
        assert!(
            link.host_events
                .contains(&TransportEvent::ChannelRejected("input".into())),
            "{:?}",
            link.host_events
        );
        assert!(
            !link
                .host_events
                .contains(&TransportEvent::ChannelOpen("input".into()))
        );
        assert!(
            link.closed_by_host.contains(&"input".to_string()),
            "the refused channel is closed"
        );

        link.authenticated.store(true, Ordering::Release);
        let input = link.client_opens("input");
        link.run(Duration::from_millis(60));
        assert!(
            link.host_events
                .contains(&TransportEvent::ChannelOpen("input".into())),
            "{:?}",
            link.host_events
        );
        link.client_writes_to(input, &[5, 6, 7]);
        link.run(Duration::from_millis(30));
        assert!(link.host_events.contains(&TransportEvent::Input {
            data: vec![5, 6, 7],
            binary: true
        }));

        // Revoking the session closes the channel on the next message.
        link.authenticated.store(false, Ordering::Release);
        link.client_writes_to(input, &[1]);
        link.run(Duration::from_millis(30));
        assert!(!link.host_events.contains(&TransportEvent::Input {
            data: vec![1],
            binary: true
        }));
        let rejected = link
            .host_events
            .iter()
            .filter(|e| **e == TransportEvent::ChannelRejected("input".into()))
            .count();
        assert_eq!(rejected, 2, "the revoked channel is refused and closed");
    }

    #[test]
    fn unknown_channel_labels_are_closed() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.authenticated.store(true, Ordering::Release);
        link.client_opens("shell");
        // A second channel that claims a host label is refused too.
        link.client_opens("control");
        link.run(Duration::from_millis(60));
        assert!(
            link.host_events
                .contains(&TransportEvent::ChannelRejected("shell".into())),
            "{:?}",
            link.host_events
        );
        assert!(
            link.host_events
                .contains(&TransportEvent::ChannelRejected("control".into())),
            "{:?}",
            link.host_events
        );
        assert!(link.closed_by_host.contains(&"shell".to_string()));
    }

    fn nack_run(rtx: bool) -> (Vec<Rtp>, usize) {
        let mut s = spec(VideoCodec::H264, false);
        s.rtx = rtx;
        let mut link = Link::connected(s);
        link.drop_rtp = Some(link.rtp_seen + 3);
        link.video(h264_frame(true, 20_000), 0, true);
        link.run(Duration::from_millis(600));
        let frames = link.media.iter().filter(|m| m.pt == 96).count();
        (link.rtp, frames)
    }

    #[test]
    fn a_lost_packet_is_resent_only_when_rtx_is_negotiated() {
        let (with_rtx, _) = nack_run(true);
        assert!(
            with_rtx.iter().any(|p| p.pt == 97),
            "with RTX the NACK is answered"
        );

        // Without an RTX payload type str0m marks every packet non-nackable, so a NACK would be
        // ignored; the offer doesn't even negotiate NACK, so receivers send a PLI instead.
        let (without, frames) = nack_run(false);
        assert!(
            without.iter().all(|p| p.pt == 96),
            "no retransmission without RTX"
        );
        let mut seqs: Vec<u16> = without.iter().map(|p| p.seq).collect();
        let total = seqs.len();
        seqs.dedup();
        assert_eq!(seqs.len(), total, "no duplicate sequence numbers");
        assert_eq!(frames, 0, "the frame with the hole is never completed");
    }

    #[test]
    fn av1_temporal_delimiters_are_removed_and_the_rest_kept() {
        let keyframe = av1_frame(true, 300);
        assert_eq!(
            strip_av1_temporal_delimiters(&keyframe).unwrap().as_ref(),
            &keyframe[2..]
        );
        let plain = obu(6, &filler(200, 1));
        assert!(matches!(
            strip_av1_temporal_delimiters(&plain).unwrap(),
            Cow::Borrowed(_)
        ));
        // A TD in the middle (two temporal units in one buffer) goes too.
        let two = [plain.clone(), TD.to_vec(), plain.clone()].concat();
        assert_eq!(
            strip_av1_temporal_delimiters(&two).unwrap().as_ref(),
            [plain.clone(), plain].concat()
        );
        // An OBU without a size field takes the rest.
        let tail = [obu(1, &[1, 2, 3]), vec![(6 << 3), 9, 9, 9]].concat();
        assert!(matches!(
            strip_av1_temporal_delimiters(&tail).unwrap(),
            Cow::Borrowed(_)
        ));
        // Malformed input is an error, not a guess.
        assert!(strip_av1_temporal_delimiters(&[0x32, 0x40, 1, 2]).is_err());
        assert!(strip_av1_temporal_delimiters(&[0x80, 0]).is_err());
    }

    fn rtp_packet(len: usize) -> Outgoing {
        let mut data = vec![0x60u8; len];
        data[0] = 0x80;
        Outgoing {
            destination: CLIENT.parse().unwrap(),
            data,
            paced: true,
        }
    }

    fn control_packet() -> Outgoing {
        Outgoing {
            destination: CLIENT.parse().unwrap(),
            data: vec![0x16; 100],
            paced: false,
        }
    }

    const PERIOD: Duration = Duration::from_micros(16_667);

    /// Paced at a configured cap, in kbps, on a route of unknown speed.
    fn capped(kbps: i64) -> Pacing {
        Pacing::Paced {
            configured_kbps: kbps,
            link_bps: 0,
            wireless: false,
        }
    }

    #[test]
    fn the_loss_switch_drops_only_video_and_about_the_share_asked_for() {
        let queue =
            || SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()).with_loss(10);
        let (mut lossy, mut clean) = (
            queue(),
            SendQueue::new(Pacing::Off, 30_000, PERIOD, false, Instant::now()),
        );
        let video = rtp_packet(1200);
        let lost = (0..100_000)
            .filter(|_| lossy.lost_in_the_air(&video))
            .count();
        assert!((800..=1200).contains(&lost), "{lost} of 100000 at 1%");
        assert!((0..1000).all(|_| !lossy.lost_in_the_air(&control_packet())));
        assert!((0..1000).all(|_| !clean.lost_in_the_air(&video)));
        // The seed is fixed: a second queue loses the same packets.
        let mut second = queue();
        let again = (0..100_000)
            .filter(|_| second.lost_in_the_air(&video))
            .count();
        assert_eq!(again, lost);
    }

    #[test]
    fn the_pacer_sends_one_millisecond_of_media_per_group() {
        let now = Instant::now();
        // 12 Mbps is 1500 bytes of wire traffic a millisecond: one 1100-byte datagram (1166 on the
        // wire) fits, a second does not.
        let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
        for _ in 0..5 {
            queue.push(rtp_packet(1100));
        }
        queue.push(control_packet());
        let first = queue.next_batch(now);
        assert_eq!(
            first.packets.len(),
            2,
            "the control datagram and one media datagram"
        );
        assert!(!first.packets[0].paced && first.packets[1].paced);
        queue.sent(now, &first);
        assert!(
            queue.next_batch(now).packets.is_empty(),
            "the next group waits"
        );
        let due = queue.due(now).unwrap();
        assert!(
            due > now && due <= now + Duration::from_millis(1),
            "{:?}",
            due - now
        );
        let second = queue.next_batch(due);
        assert_eq!(second.paced_packets, 1);
    }

    #[test]
    fn a_large_frame_raises_the_wireless_pacing_rate_to_what_it_needs() {
        let now = Instant::now();
        let wireless = Pacing::Paced {
            configured_kbps: 0,
            link_bps: 0,
            wireless: true,
        };
        // A 1 Mbps stream is paced at 2 Mbps (one datagram a group) until a frame needs more.
        let packets = |note: Option<usize>| {
            let mut queue = SendQueue::new(wireless, 1_000, PERIOD, false, now);
            if let Some(bytes) = note {
                queue.note_frame(bytes);
            }
            for _ in 0..40 {
                queue.push(rtp_packet(1100));
            }
            queue.next_batch(now).paced_packets
        };
        assert_eq!(packets(None), 1);
        // 60 datagrams of 1166 bytes per frame period is 33.6 Mbps; with the quarter on top, 42.
        assert_eq!(packets(Some(60 * 1166)), 4);
    }

    #[test]
    fn the_ceiling_is_a_real_cap() {
        let now = Instant::now();
        let rate = |pacing: Pacing, frame: usize| {
            let mut queue = SendQueue::new(pacing, 20_000, PERIOD, false, now);
            queue.note_frame(frame);
            queue.rate()
        };
        // A 300 KB frame (144 Mbps at 60 fps) on a 100 Mbps link is held to 4/5 of the link.
        let link = |link_bps, wireless| Pacing::Paced {
            configured_kbps: 0,
            link_bps,
            wireless,
        };
        let huge = 300_000;
        assert_eq!(rate(link(100_000_000, false), huge), 80_000_000);
        assert_eq!(rate(link(100_000_000, true), huge), 80_000_000);
        // No link known: wired 800 Mbps, and wireless is clamped to the same.
        assert_eq!(rate(link(0, false), huge), 800_000_000);
        assert_eq!(rate(link(0, true), huge * 10), 800_000_000);
        // A configured cap holds, whatever the frame needs.
        assert_eq!(rate(capped(50_000), huge), 50_000_000);
        // Never below what the stream itself needs (1.1 times its bitrate).
        assert_eq!(rate(capped(5_000), 0), 22_000_000);
        assert_eq!(rate(Pacing::Off, huge), u64::MAX);
    }

    #[test]
    fn a_late_wake_catches_up_but_a_stall_earns_a_bounded_burst() {
        let now = Instant::now();
        // 12 Mbps with 1100-byte datagrams: one datagram, 0.78 ms of wire time, per group.
        let groups_at = |late: Duration| {
            let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, now);
            for _ in 0..200 {
                queue.push(rtp_packet(1100));
            }
            let first = queue.next_batch(now);
            queue.sent(now, &first);
            // The timer fires `late` after the first group was due again.
            let at = queue.due(now).unwrap() + late;
            let mut sent = 0;
            loop {
                let batch = queue.next_batch(at);
                if batch.paced_packets == 0 {
                    return sent;
                }
                queue.sent(at, &batch);
                sent += batch.paced_packets;
            }
        };
        let on_time = groups_at(Duration::ZERO);
        let late_2ms = groups_at(Duration::from_millis(2));
        let stalled = groups_at(Duration::from_millis(500));
        assert_eq!(on_time, 1);
        assert!(
            late_2ms >= 3,
            "a 2 ms late wake sends the groups it missed: {late_2ms}"
        );
        assert!(
            (2..=8).contains(&stalled),
            "a 500 ms stall earns {CATCH_UP_CREDIT:?} of credit, not a burst of 400 groups: {stalled}"
        );
    }

    #[test]
    fn a_backlog_beyond_the_cap_is_detected_but_a_lone_keyframe_is_not() {
        let now = Instant::now();
        // 80 Mbps: 50 ms of backlog is 500 KB.
        let mut queue = SendQueue::new(capped(80_000), 20_000, PERIOD, false, now);
        // A 1 MB keyframe is twice the 50 ms budget, but it is one frame.
        queue.note_frame(1_000_000);
        for _ in 0..860 {
            queue.push(rtp_packet(1100));
        }
        assert!(!queue.backlog_exceeded());
        // Two more of them (3 x frame > 2 x frame) is a pile-up.
        for _ in 0..1800 {
            queue.push(rtp_packet(1100));
        }
        assert!(queue.backlog_exceeded());
        assert_eq!(queue.drop_paced(), 2660);
        assert!(!queue.backlog_exceeded() && queue.is_empty());
        // Small frames: the 50 ms budget alone decides.
        queue.note_frame(20_000);
        for _ in 0..400 {
            queue.push(rtp_packet(1100));
        }
        assert!(!queue.backlog_exceeded());
        for _ in 0..200 {
            queue.push(rtp_packet(1100));
        }
        assert!(queue.backlog_exceeded(), "{}", queue.paced_wire_bytes);
    }

    #[test]
    fn pacing_off_hands_everything_over_at_once() {
        let now = Instant::now();
        let mut queue = SendQueue::new(Pacing::Off, 0, PERIOD, false, now);
        for _ in 0..50 {
            queue.push(rtp_packet(1100));
        }
        assert!(!queue.backlog_exceeded());
        assert_eq!(queue.next_batch(now).packets.len(), 50);
        assert!(queue.is_empty());
    }

    fn detached(
        options: TransportOptions,
    ) -> (
        MediaHandle,
        mpsc::Receiver<Command>,
        Arc<Mutex<Vec<IdrReason>>>,
    ) {
        let idr = Arc::new(Mutex::new(Vec::new()));
        let sink = idr.clone();
        let hooks = Hooks::new(move |r| sink.lock().unwrap().push(r), |_| {});
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            wake: Signal::new().unwrap(),
            authenticated: Arc::new(AtomicBool::new(false)),
            pending_video: AtomicUsize::new(0),
            pending_audio: AtomicUsize::new(0),
            pending_datagrams: AtomicUsize::new(0),
            need_keyframe: AtomicBool::new(true),
            last_idr_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            counters: Counters::default(),
            hooks,
            options,
            peer: std::sync::Mutex::new(None),
        });
        (
            MediaHandle {
                tx,
                shared,
                threads: Vec::new(),
                local: HOST.parse().unwrap(),
                audio_layout: None,
            },
            rx,
            idr,
        )
    }

    fn frame(keyframe: bool) -> VideoFrame {
        VideoFrame {
            data: vec![0; 10],
            capture: Instant::now(),
            keyframe,
        }
    }

    #[test]
    fn a_full_frame_queue_drops_and_asks_for_a_keyframe_without_blocking() {
        let (handle, rx, idr) = detached(TransportOptions {
            video_queue: 2,
            ..TransportOptions::default()
        });
        assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
        assert_eq!(handle.submit_video(frame(false)), Submit::Queued);
        // The media thread isn't draining: the third frame is dropped at once.
        assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
        assert_eq!(idr.lock().unwrap().as_slice(), [IdrReason::QueueFull]);
        // Later delta frames would reference the dropped one, so they are dropped too.
        while let Ok(Command::Video(_)) = rx.try_recv() {
            handle.shared.pending_video.fetch_sub(1, Ordering::AcqRel);
        }
        assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
        assert_eq!(
            handle.submit_video(frame(true)),
            Submit::Queued,
            "a keyframe goes through"
        );
        assert_eq!(
            handle.submit_video(frame(false)),
            Submit::Queued,
            "and delta frames follow it"
        );
        assert_eq!(handle.stats().video_dropped, 2);
    }

    #[test]
    fn audio_queue_overflow_drops_without_blocking() {
        let (handle, _rx, idr) = detached(TransportOptions {
            audio_queue: 1,
            ..TransportOptions::default()
        });
        let packet = || AudioPacket {
            data: vec![1],
            capture: Instant::now(),
            position: 0,
        };
        assert_eq!(handle.submit_audio(packet()), Submit::Queued);
        assert_eq!(handle.submit_audio(packet()), Submit::Dropped);
        assert!(
            idr.lock().unwrap().is_empty(),
            "a lost audio packet needs no keyframe"
        );
        assert_eq!(handle.stats().audio_dropped, 1);
    }

    #[test]
    fn the_audio_sender_shares_the_handles_queue_and_counters() {
        let (handle, rx, _idr) = detached(TransportOptions {
            audio_queue: 1,
            ..TransportOptions::default()
        });
        assert!(
            handle.audio_sender().is_none(),
            "no layout, no audio end: stereo and surround must both say what they encode"
        );
        let layout = OpusLayout::select(6, false, None).unwrap();
        let handle = handle.with_audio_layout(layout.clone());
        let sender = handle.audio_sender().unwrap();
        assert_eq!(sender.layout(), &layout);
        let packet = || AudioPacket {
            data: vec![7],
            capture: Instant::now(),
            position: 0,
        };
        assert_eq!(sender.submit(packet()), Submit::Queued);
        assert_eq!(
            handle.submit_audio(packet()),
            Submit::Dropped,
            "one queue, whichever end fills it"
        );
        assert!(matches!(rx.try_recv(), Ok(Command::Audio(p)) if p.data == [7]));
        assert_eq!(handle.stats().audio_dropped, 1);
        // The sender outlives nothing: with the handle's threads gone a packet is dropped.
        drop(rx);
        handle.shared.pending_audio.store(0, Ordering::Release);
        assert_eq!(sender.submit(packet()), Submit::Dropped);
        assert_eq!(handle.stats().audio_dropped, 2);
    }

    #[test]
    fn the_threads_start_and_stop_on_a_real_socket() {
        let spec = spec(VideoCodec::H264, false);
        let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
        let local = socket.local_addr().unwrap();
        let (mut rtc, offer) = host_offer(&spec, local);
        let mut peer = client(&spec, CLIENT.parse().unwrap());
        let text = answer(&mut peer, &offer.sdp);
        let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
        let session = Session {
            rtc,
            ids,
            spec: &spec,
            authenticated: Arc::new(AtomicBool::new(false)),
        };
        let mut handle = MediaHandle::spawn(
            socket,
            local,
            session,
            TransportOptions::default(),
            Hooks::new(|_| {}, |_| {}),
        )
        .unwrap();
        // Not connected: the frame is accepted by the queue and then discarded by the thread.
        assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
        let started = Instant::now();
        handle.close();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A STUN binding request (RFC 8489) with only a USERNAME: no integrity, so no real peer.
    fn forged_stun(username: &str) -> Vec<u8> {
        let mut attr = username.as_bytes().to_vec();
        let length = attr.len() as u16;
        attr.resize(attr.len().div_ceil(4) * 4, 0);
        let mut message = vec![0, 1];
        message.extend(((attr.len() + 4) as u16).to_be_bytes());
        message.extend([0x21, 0x12, 0xa4, 0x42]);
        message.extend([7u8; 12]);
        message.extend([0, 6]);
        message.extend(length.to_be_bytes());
        message.extend(attr);
        message
    }

    #[test]
    fn audio_goes_out_unpaced_beside_a_paced_keyframe() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.video(h264_frame(true, 60_000), 0, true);
        link.audio(filler(160, 1), 0, 0);
        // str0m hands a frame over across a few polls; collect what the engine releases.
        let mut out = Vec::new();
        for ms in 0..20 {
            link.host
                .advance(link.now + Duration::from_millis(ms))
                .unwrap();
            out.extend(link.host.take_outgoing());
        }
        let by_pt = |pt: u8| -> Vec<&Outgoing> {
            out.iter()
                .filter(|o| is_media_rtp(&o.data) && o.data[1] & 0x7f == pt)
                .collect()
        };
        let (video, audio) = (by_pt(VIDEO_PT), by_pt(AUDIO_PT));
        assert!(
            video.len() > 20 && audio.len() == 1,
            "{} {}",
            video.len(),
            audio.len()
        );
        assert!(video.iter().all(|o| o.paced), "video waits for the pacer");
        assert!(audio.iter().all(|o| !o.paced), "audio does not");
        // Through a queue still holding the keyframe, the audio leaves in the first batch.
        let mut queue = SendQueue::new(capped(12_000), 1_000, PERIOD, false, link.now);
        for o in out {
            queue.push(o);
        }
        let first = queue.next_batch(link.now);
        assert!(
            first
                .packets
                .iter()
                .any(|o| !o.paced && o.data[1] & 0x7f == AUDIO_PT)
        );
        assert!(first.paced_packets <= 2, "{}", first.paced_packets);
    }

    #[test]
    fn foreign_and_spoofed_datagrams_cannot_end_or_hijack_the_session() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let client: SocketAddr = CLIENT.parse().unwrap();
        let attacker: SocketAddr = "203.0.113.9:4444".parse().unwrap();
        let rtp = {
            let mut p = vec![0x80, 96, 0, 1, 0, 0, 0, 1, 0xde, 0xad, 0xbe, 0xef];
            p.extend(filler(200, 1));
            p
        };
        let rtcp = vec![0x80, 201, 0, 1, 1, 2, 3, 4];
        let dtls = [
            vec![22, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4],
            filler(4, 1),
        ]
        .concat();
        let own = format!("{}:abcd", link.host_ufrag);
        let junk: Vec<Vec<u8>> = vec![
            rtp,
            rtcp,
            dtls,
            vec![0xff; 3000],
            vec![],
            vec![0],
            forged_stun("nobody:else"),
            forged_stun(&own),
        ];
        let now = link.now;
        for from in [attacker, client] {
            for data in &junk {
                // Whatever it is called, it is neither fatal nor a reason to stop.
                link.host.handle_datagram(now, from, data);
            }
        }
        // Foreign sources are refused outright, apart from STUN carrying our own ufrag.
        for data in &junk[..7] {
            assert_eq!(
                link.host.handle_datagram(now, attacker, data),
                Datagram::Rejected,
                "{:?}",
                &data[..data.len().min(8)]
            );
        }
        link.host.advance(link.now).unwrap();
        let out = link.host.take_outgoing();
        assert!(
            out.iter().all(|o| o.destination == client),
            "nothing is sent to the stranger: {:?}",
            out.iter().map(|o| o.destination).collect::<Vec<_>>()
        );
        assert!(link.host.is_connected() && !link.host.is_closed());
        let (rejected, _) = link.host.dropped_datagrams();
        assert!(rejected >= 14, "{rejected}");

        // The real client still gets its picture, to the address it nominated.
        link.video(h264_frame(true, 5_000), 0, true);
        link.run(Duration::from_millis(40));
        assert!(link.media.iter().any(|m| m.pt == 96), "video still flows");
        assert!(
            !link.host_events.contains(&TransportEvent::Closed)
                && !link.host_events.contains(&TransportEvent::Disconnected),
            "{:?}",
            link.host_events
        );
    }

    #[test]
    fn an_ice_outage_is_surfaced_and_ends_the_session_after_the_timeout() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.video(h264_frame(true, 2_000), 0, true);
        link.run(Duration::from_millis(20));
        link.host_events.clear();
        link.idr.clear();
        link.partition = true;
        let started = link.now;
        while !link.host.is_disconnected() {
            link.step(Duration::from_millis(100));
            assert!(
                link.now - started < Duration::from_secs(60),
                "ICE never noticed the outage"
            );
        }
        let went_down = link.now;
        assert!(link.host_events.contains(&TransportEvent::Disconnected));
        assert!(!link.host.is_connected() && !link.host.is_closed());
        // While down, nothing is written, so the next frames wait for a keyframe.
        let frame = VideoFrame {
            data: h264_frame(true, 1_000),
            capture: link.at(0),
            keyframe: true,
        };
        assert!(!link.host.write_video(link.now, &frame).unwrap());

        while !link.host.is_closed() {
            link.step(Duration::from_millis(50));
            assert!(
                link.now - went_down < Duration::from_secs(10),
                "never closed"
            );
        }
        let waited = link.now - went_down;
        assert!(
            waited >= DISCONNECT_TIMEOUT - Duration::from_millis(200)
                && waited <= DISCONNECT_TIMEOUT + Duration::from_millis(300),
            "{waited:?}"
        );
    }

    #[test]
    fn an_outage_that_ends_in_time_resumes_with_a_keyframe_request() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.partition = true;
        let started = link.now;
        while !link.host.is_disconnected() {
            link.step(Duration::from_millis(100));
            assert!(link.now - started < Duration::from_secs(60));
        }
        link.host_events.clear();
        link.idr.clear();
        link.partition = false;
        for _ in 0..25 {
            link.step(Duration::from_millis(100));
            if link.host.is_connected() {
                break;
            }
        }
        assert!(link.host.is_connected(), "{:?}", link.host_events);
        assert!(link.host_events.contains(&TransportEvent::Reconnected));
        assert!(link.idr.contains(&IdrReason::Reconnected), "{:?}", link.idr);
    }

    #[test]
    fn a_channel_closed_by_the_client_is_reported_and_stops_sends() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.authenticated.store(true, Ordering::Release);
        let input = link.client_opens("input");
        link.run(Duration::from_millis(60));
        assert!(
            link.host_events
                .contains(&TransportEvent::ChannelOpen("input".into()))
        );
        assert!(link.host.send_control(b"x", false).unwrap());
        for label in ["control", "input"] {
            let id = if label == "input" {
                input
            } else {
                link.client_channel(label)
            };
            link.client.direct_api().close_data_channel(id);
            link.run(Duration::from_millis(60));
            assert!(
                link.host_events.contains(&TransportEvent::ChannelClosed {
                    label: label.into()
                }),
                "{label}: {:?}",
                link.host_events
            );
        }
        assert!(
            !link.host.send_control(b"x", false).unwrap(),
            "a closed channel takes nothing"
        );
        // The gamepad channel is untouched.
        assert!(link.host.send_gamepad(&[1], true).unwrap());
    }

    #[test]
    fn the_input_channel_must_be_ordered_and_reliable() {
        use str0m::channel::ChannelConfig;
        let cases: [(&str, bool, Reliability, bool); 4] = [
            ("ordered and reliable", true, Reliability::Reliable, true),
            ("unordered", false, Reliability::Reliable, false),
            (
                "unreliable",
                true,
                Reliability::MaxRetransmits { retransmits: 0 },
                false,
            ),
            (
                "lossy and unordered",
                false,
                Reliability::MaxPacketLifetime { lifetime: 50 },
                false,
            ),
        ];
        for (name, ordered, reliability, accepted) in cases {
            let mut link = Link::connected(spec(VideoCodec::H264, false));
            link.authenticated.store(true, Ordering::Release);
            let mut api = link.client.sdp_api();
            api.add_channel_with_config(ChannelConfig {
                label: "input".into(),
                ordered,
                reliability,
                ..Default::default()
            });
            assert!(api.apply().is_none());
            link.run(Duration::from_millis(60));
            let open = link
                .host_events
                .contains(&TransportEvent::ChannelOpen("input".into()));
            let rejected = link
                .host_events
                .contains(&TransportEvent::ChannelRejected("input".into()));
            assert_eq!((open, rejected), (accepted, !accepted), "{name}");
        }
    }

    #[test]
    fn messages_that_cannot_be_sent_are_counted() {
        let (handle, _rx, _idr) = detached(TransportOptions::default());
        let shared = &handle.shared;
        note_channel_send(shared, "control", Ok(true));
        note_channel_send(shared, "control", Ok(false));
        note_channel_send(shared, "gamepad", Ok(false));
        note_channel_send(shared, "gamepad", Err(anyhow!("channel write failed")));
        let stats = handle.stats();
        assert_eq!((stats.channel_sent, stats.channel_dropped), (1, 3));
        // Before the channels open, the engine itself says no rather than pretending.
        let mut link = Link::new(spec(VideoCodec::H264, false));
        assert!(!link.host.send_control(b"x", false).unwrap());
        assert!(!link.host.send_gamepad(&[1], true).unwrap());
    }

    #[test]
    fn no_frame_but_a_keyframe_leaves_before_the_first_one() {
        let (handle, rx, _idr) = detached(TransportOptions::default());
        assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
        assert_eq!(handle.stats().video_dropped, 1);
        assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
        assert_eq!(handle.submit_video(frame(false)), Submit::Queued);
        assert_eq!(rx.try_iter().count(), 2);
    }

    fn frame_of(data: Vec<u8>, keyframe: bool) -> VideoFrame {
        VideoFrame {
            data,
            capture: Instant::now(),
            keyframe,
        }
    }

    #[test]
    fn stale_deltas_after_a_drop_are_discarded_by_the_thread() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let (handle, _rx, _idr) = detached(TransportOptions::default());
        let shared = &handle.shared;
        shared.pending_video.store(3, Ordering::Release);
        let command = |keyframe| Command::Video(frame_of(h264_frame(keyframe, 800), keyframe));
        handle_command(&mut link.host, shared, command(false));
        let stats = handle.stats();
        assert_eq!((stats.video_frames, stats.video_dropped), (0, 1));
        handle_command(&mut link.host, shared, command(true));
        assert!(!shared.need_keyframe.load(Ordering::Acquire));
        handle_command(&mut link.host, shared, command(false));
        let stats = handle.stats();
        assert_eq!((stats.video_frames, stats.video_dropped), (2, 1));
    }

    #[test]
    fn a_pacer_pile_up_drops_the_backlog_and_asks_for_a_keyframe() {
        let (handle, _rx, idr) = detached(TransportOptions::default());
        let shared = &handle.shared;
        shared.need_keyframe.store(false, Ordering::Release);
        let now = Instant::now();
        let mut queue = SendQueue::new(capped(8_000), 5_000, PERIOD, false, now);
        queue.note_frame(10_000);
        for _ in 0..30 {
            queue.push(rtp_packet(1100));
        }
        enforce_backlog(&mut queue, shared);
        assert!(idr.lock().unwrap().is_empty(), "30 KB is no backlog yet");
        for _ in 0..300 {
            queue.push(rtp_packet(1100));
        }
        enforce_backlog(&mut queue, shared);
        assert_eq!(idr.lock().unwrap().as_slice(), [IdrReason::PacerBacklog]);
        assert!(shared.need_keyframe.load(Ordering::Acquire));
        assert_eq!(handle.stats().paced_dropped, 330);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_datagram_burst_beyond_the_queue_is_dropped_not_buffered() {
        let (handle, rx, _idr) = detached(TransportOptions {
            datagram_queue: 3,
            ..TransportOptions::default()
        });
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let target = socket.local_addr().unwrap();
        let (tx, shared) = (handle.tx.clone(), handle.shared.clone());
        let thread = std::thread::spawn(move || receive_loop(socket, tx, shared));
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        for i in 0..10u8 {
            sender.send_to(&[i; 40], target).unwrap();
        }
        wait_for("ten datagrams", || handle.stats().datagrams_received == 10);
        let stats = handle.stats();
        assert_eq!(stats.datagrams_dropped, 7, "{stats:?}");
        assert_eq!(rx.try_iter().count(), 3, "only the first three are queued");
        handle.shared.stop.store(true, Ordering::Release);
        thread.join().unwrap();
    }

    fn ice(link: &mut Link, state: IceConnectionState) {
        link.host
            .on_event(link.now, Event::IceConnectionStateChange(state));
    }

    #[test]
    fn checking_after_an_outage_is_not_back_and_does_not_reset_the_timeout() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.collect_host();
        link.host_events.clear();
        link.idr.clear();
        ice(&mut link, IceConnectionState::Disconnected);
        let down_at = link.host.disconnected_since.expect("down");
        link.now += Duration::from_secs(1);
        ice(&mut link, IceConnectionState::Checking);
        assert_eq!(
            link.host.disconnected_since,
            Some(down_at),
            "the clock runs on"
        );
        assert!(link.host.is_disconnected());
        link.collect_host();
        assert_eq!(link.host_events, [TransportEvent::Disconnected]);
        assert!(link.idr.is_empty(), "{:?}", link.idr);
        // Only a usable pair is back, and it asks for exactly one keyframe.
        ice(&mut link, IceConnectionState::Completed);
        ice(&mut link, IceConnectionState::Connected);
        link.collect_host();
        assert!(!link.host.is_disconnected());
        assert_eq!(
            link.host_events,
            [TransportEvent::Disconnected, TransportEvent::Reconnected]
        );
        assert_eq!(link.idr, [IdrReason::Reconnected]);
    }

    #[test]
    fn checking_after_connecting_counts_as_down() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        link.collect_host();
        link.host_events.clear();
        // A Wi-Fi handover: Completed, then ICE goes looking for a new pair.
        ice(&mut link, IceConnectionState::Completed);
        ice(&mut link, IceConnectionState::Checking);
        assert!(link.host.is_disconnected());
        assert!(!link.host.is_connected());
        link.collect_host();
        assert_eq!(link.host_events, [TransportEvent::Disconnected]);
        // The timeout runs from the first sign of trouble and ends the session.
        link.now += DISCONNECT_TIMEOUT + Duration::from_millis(1);
        link.host.advance(link.now).unwrap();
        assert!(link.host.is_closed());
    }

    #[test]
    fn checking_before_the_session_connected_is_not_an_outage() {
        let mut link = Link::new(spec(VideoCodec::H264, false));
        ice(&mut link, IceConnectionState::Checking);
        ice(&mut link, IceConnectionState::Disconnected);
        assert!(!link.host.is_disconnected());
        link.collect_host();
        assert!(link.host_events.is_empty());
    }

    #[test]
    fn a_keyframe_cannot_overwrite_the_threads_demand_for_the_next_one() {
        let (handle, rx, _idr) = detached(TransportOptions::default());
        let shared = handle.shared.clone();
        let processed = Arc::new(AtomicUsize::new(0));
        let thread = {
            let (shared, processed) = (shared.clone(), processed.clone());
            std::thread::spawn(move || {
                // The media thread, discarding every keyframe at once.
                while let Ok(Command::Video(_)) = rx.recv() {
                    shared.pending_video.fetch_sub(1, Ordering::AcqRel);
                    shared.need_keyframe.store(true, Ordering::Release);
                    processed.fetch_add(1, Ordering::Release);
                }
            })
        };
        for i in 1..=20_000 {
            shared.need_keyframe.store(true, Ordering::Release);
            assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
            while processed.load(Ordering::Acquire) < i {
                std::hint::spin_loop();
            }
            assert!(
                shared.need_keyframe.load(Ordering::Acquire),
                "round {i}: the clear overwrote the thread's demand"
            );
        }
        drop(handle);
        thread.join().unwrap();
    }

    #[test]
    fn receive_errors_only_back_off_when_they_persist() {
        let mut consecutive = 0;
        for _ in 1..RECV_ERRORS_BEFORE_BACKOFF {
            assert_eq!(recv_error_backoff(&mut consecutive), None);
        }
        assert_eq!(
            recv_error_backoff(&mut consecutive),
            Some(RECV_ERROR_BACKOFF)
        );
        assert_eq!(
            recv_error_backoff(&mut consecutive),
            Some(RECV_ERROR_BACKOFF)
        );
        // A datagram (or a quiet timeout) in between starts the count again.
        consecutive = 0;
        assert_eq!(recv_error_backoff(&mut consecutive), None);
    }

    #[test]
    fn a_media_thread_that_cannot_start_releases_the_port() {
        let (handle, _rx, _idr) = detached(TransportOptions::default());
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let (tx, _keep) = mpsc::channel::<Command>();
        let shared = handle.shared.clone();
        let result = start_threads(
            &handle.shared,
            || std::thread::Builder::new().spawn(move || receive_loop(socket, tx, shared)),
            || Err(std::io::Error::other("no threads left")),
        );
        let err = format!("{:#}", result.expect_err("the media thread failed"));
        assert!(err.contains("starting the media thread"), "{err}");
        // The receive thread was stopped and joined, so its socket is closed.
        UdpSocket::bind(("127.0.0.1", port)).expect("the port is free again");
        assert!(handle.shared.stop.load(Ordering::Acquire));
    }

    /// A real MediaHandle on loopback, against a peer that never answers.
    fn real_handle(events: Arc<Mutex<Vec<TransportEvent>>>) -> (MediaHandle, SocketAddr) {
        let spec = spec(VideoCodec::H264, false);
        let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
        let local = socket.local_addr().unwrap();
        let (mut rtc, offer) = host_offer(&spec, local);
        let mut peer = client(&spec, CLIENT.parse().unwrap());
        let text = answer(&mut peer, &offer.sdp);
        let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
        let session = Session {
            rtc,
            ids,
            spec: &spec,
            authenticated: Arc::new(AtomicBool::new(false)),
        };
        let hooks = Hooks::new(|_| {}, move |e| events.lock().unwrap().push(e));
        let handle =
            MediaHandle::spawn(socket, local, session, TransportOptions::default(), hooks).unwrap();
        (handle, local)
    }

    #[test]
    fn big_and_garbage_datagrams_on_the_real_socket_do_not_end_the_session() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (mut handle, local) = real_handle(events.clone());
        let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
        // 3000 bytes overflowed the old 2 KB buffer, and Windows then fails the receive.
        for size in [3000, 9000, 60_000, 1, 40] {
            stranger.send_to(&vec![0x5a; size], local).unwrap();
        }
        wait_for("the datagrams to be read and refused", || {
            let stats = handle.stats();
            stats.datagrams_received >= 5 && stats.datagrams_rejected >= 5
        });
        std::thread::sleep(Duration::from_millis(150));
        let stats = handle.stats();
        assert_eq!(stats.recv_errors, 0, "{stats:?}");
        assert!(handle.is_running(), "{stats:?}");
        assert!(
            events.lock().unwrap().is_empty(),
            "{:?}",
            events.lock().unwrap()
        );
        handle.close();
        assert!(!handle.is_running());
    }

    #[test]
    fn a_frame_dropped_before_connecting_demands_a_keyframe_next() {
        let (mut handle, _) = real_handle(Arc::default());
        assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
        assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
        // The thread discards it (no peer yet), so the next delta is refused again.
        wait_for("the keyframe to be discarded", || {
            handle.stats().video_dropped >= 2
        });
        assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
        handle.close();
    }

    #[test]
    fn dropping_the_handle_inside_a_hook_does_not_join_itself() {
        let slot: Arc<Mutex<Option<MediaHandle>>> = Arc::default();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let spec = spec(VideoCodec::H264, false);
        let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
        let local = socket.local_addr().unwrap();
        let (mut rtc, offer) = host_offer(&spec, local);
        let mut peer = client(&spec, CLIENT.parse().unwrap());
        let text = answer(&mut peer, &offer.sdp);
        let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
        let session = Session {
            rtc,
            ids,
            spec: &spec,
            authenticated: Arc::new(AtomicBool::new(false)),
        };
        let hook_slot = slot.clone();
        let hooks = Hooks::new(
            |_| {},
            move |event| {
                if event == TransportEvent::Closed {
                    // The hook runs on the media thread, which must not wait for itself.
                    let handle = hook_slot.lock().unwrap().take();
                    drop(handle);
                    let _ = done_tx.send(());
                }
            },
        );
        let handle =
            MediaHandle::spawn(socket, local, session, TransportOptions::default(), hooks).unwrap();
        let (tx, shared) = (handle.tx.clone(), handle.shared.clone());
        *slot.lock().unwrap() = Some(handle);
        // End the media thread from outside; its closing hook then drops the last handle.
        tx.send(Command::Stop).unwrap();
        shared.notify();
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the hook returned (no self-join)");
        assert!(slot.lock().unwrap().is_none());
    }

    /// `cargo test -p butterpollo --release -- --ignored --nocapture webrtc_bench`
    #[test]
    #[ignore = "measurement"]
    fn webrtc_bench_keyframe_and_srtp_cost() {
        let mut link = Link::connected(spec(VideoCodec::H264, false));
        let keyframe = h264_frame(true, 1_000_000);
        let started = Instant::now();
        link.video(keyframe, 0, true);
        link.host.advance(link.now).unwrap();
        let out = link.host.take_outgoing();
        let elapsed = started.elapsed();
        let bytes: usize = out.iter().map(|o| o.data.len()).sum();
        println!(
            "webrtc_bench: 1 MB keyframe: {} datagrams, {} bytes, submit to last packet {:.2} ms (no pacing)",
            out.len(),
            bytes,
            elapsed.as_secs_f64() * 1000.
        );
        let rate = 800_000_000.0;
        println!(
            "webrtc_bench: at the 800 Mbps pacing ceiling the same frame leaves in {:.1} ms",
            bytes as f64 * 8. / rate * 1000.
        );

        let frames = 200;
        let size = 250_000;
        let mut total = 0usize;
        let mut spent = Duration::ZERO;
        for i in 0..frames {
            let data = h264_frame(false, size);
            let started = Instant::now();
            link.video(data, 10 * i, false);
            link.host.advance(link.now).unwrap();
            total += link
                .host
                .take_outgoing()
                .iter()
                .map(|o| o.data.len())
                .sum::<usize>();
            spent += started.elapsed();
        }
        let mbit = total as f64 * 8. / 1e6;
        println!(
            "webrtc_bench: packetize and SRTP (the built crypto provider): {:.1} us per KB, {:.2} s of one core per 1000 Mbit, so {:.1}% of a core at 100 Mbps",
            spent.as_secs_f64() * 1e6 / (total as f64 / 1000.),
            spent.as_secs_f64() / mbit * 1000.,
            spent.as_secs_f64() / mbit * 100. * 100.
        );
    }
}
