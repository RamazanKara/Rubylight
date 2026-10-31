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
/// own. Other OBUs pass byte for byte, moved down in the frame's own buffer.
pub fn strip_av1_temporal_delimiters(mut data: Vec<u8>) -> Result<Vec<u8>> {
    const OBU_TEMPORAL_DELIMITER: u8 = 2;
    // Where the next kept OBU goes.
    let mut kept = 0;
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
        if obu_type != OBU_TEMPORAL_DELIMITER {
            if kept != at {
                data.copy_within(at..end, kept);
            }
            kept += end - at;
        }
        at = end;
    }
    data.truncate(kept);
    Ok(data)
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

    /// The datagrams str0m wrote since the last call, leaving the queue's allocation for the next.
    pub fn drain_outgoing(&mut self) -> std::collections::vec_deque::Drain<'_, Outgoing> {
        self.outgoing.drain(..)
    }

    #[cfg(test)]
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.drain_outgoing().collect()
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
    /// The frame's buffer goes to str0m as it is, not copied.
    pub fn write_video(&mut self, now: Instant, frame: VideoFrame) -> Result<bool> {
        if !self.is_connected() {
            return Ok(false);
        }
        let packets = frame.data.len().div_ceil(RTP_PAYLOAD_TARGET).max(1);
        let data = match self.codec {
            VideoCodec::Av1 => strip_av1_temporal_delimiters(frame.data)?,
            _ => frame.data,
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
                data,
            )
            .map_err(|e| anyhow!("video write failed: {e}"))?;
        let wire = packets * (DATAGRAM_TARGET + network_pacing::overhead(self.local.is_ipv6()));
        self.frame_wire_bytes = self.frame_wire_bytes.max(wire);
        Ok(true)
    }

    /// One Opus packet. Stereo and multistream payloads are not looked at.
    pub fn write_audio(&mut self, now: Instant, packet: AudioPacket) -> Result<bool> {
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
                packet.data,
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
    /// promises; `send_due` counts it as a simulated loss, not as sent.
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

    #[cfg(test)]
    pub fn next_batch(&mut self, now: Instant) -> Batch {
        let mut batch = Batch::default();
        self.fill_batch(now, &mut batch);
        batch
    }

    /// Everything unpaced, plus (when due) up to one millisecond of media at the pacing rate and
    /// always at least one datagram. `batch` is emptied first; its allocation is reused.
    pub fn fill_batch(&mut self, now: Instant, batch: &mut Batch) {
        batch.packets.clear();
        batch.packets.extend(self.immediate.drain(..));
        (batch.paced_bytes, batch.paced_packets, batch.rate_bps) = (0, 0, 0);
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
    /// Video datagrams the loss switch (`webrtc_loss_permille`) threw away instead of sending.
    simulated_losses: AtomicU64,
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
            simulated_losses: get(&c.simulated_losses),
        }
    }

    /// The video datagrams the loss switch has thrown away so far, for the `stream timings` line;
    /// None while the switch is off.
    pub fn simulated_losses(&self) -> Option<u64> {
        (self.shared.options.loss_permille > 0).then(|| {
            self.shared
                .counters
                .simulated_losses
                .load(Ordering::Relaxed)
        })
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
        // The datagrams being sent, kept between batches.
        let mut batch = Batch::default();
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
            for out in engine.drain_outgoing() {
                queue.push(out);
            }
            enforce_backlog(&mut queue, &shared);
            dispatch(&mut engine, &shared);
            if engine.is_closed() {
                return Ok(());
            }
            send_due(&socket, &mut queue, &mut batch, &shared);
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
            let (bytes, keyframe) = (frame.data.len() as u64, frame.keyframe);
            match engine.write_video(now, frame) {
                Ok(true) => {
                    counters.video_frames.fetch_add(1, Ordering::Relaxed);
                    counters.video_bytes.fetch_add(bytes, Ordering::Relaxed);
                    if keyframe {
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
            match engine.write_audio(now, packet) {
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

fn send_due(socket: &UdpSocket, queue: &mut SendQueue, batch: &mut Batch, shared: &Shared) {
    let mut now = Instant::now();
    // The pacing timer wakes up to its learned lead early; that is on schedule.
    while queue.due(now).is_some_and(|due| due <= now + EARLY_SEND) {
        queue.fill_batch(now + EARLY_SEND, batch);
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
                shared
                    .counters
                    .simulated_losses
                    .fetch_add(1, Ordering::Relaxed);
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
        queue.sent(now, batch);
    }
    // The sent datagrams' buffers go now, not with the next batch.
    batch.packets.clear();
}

#[cfg(test)]
pub(crate) mod tests;
