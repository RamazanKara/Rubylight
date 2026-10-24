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
use butterpollo_core::network_pacing::{self, Pacer};
use socket2::{Domain, Protocol as SockProtocol, Socket, Type};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use str0m::channel::ChannelId;
use str0m::media::{Frequency, MediaTime, Pt};
use str0m::net::{Protocol, Receive};
use str0m::{Event, Input, Output, Rtc};

/// Socket send buffer: a 4K keyframe is a few hundred KB sent in one pacing pass.
pub const SEND_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const RECV_BUFFER_BYTES: usize = 1024 * 1024;
/// Queued frames between the encoder and the media thread. At 60+ fps this is a few tens of
/// milliseconds; more would only delay the picture.
pub const DEFAULT_VIDEO_QUEUE: usize = 4;
pub const DEFAULT_AUDIO_QUEUE: usize = 64;
/// How often a stalled encoder (waiting for a keyframe) is asked again.
const IDR_REQUEST_INTERVAL: Duration = Duration::from_millis(250);
/// The UDP payload size str0m targets (`DATAGRAM_MTU_TARGET`), for the pacer's frame maths.
const DATAGRAM_TARGET: usize = 1150;
const RTP_PAYLOAD_TARGET: usize = DATAGRAM_TARGET - 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// Hand every packet to the socket as soon as str0m produces it.
    Off,
    /// 1 ms groups at `ceiling_bps`, raised when one frame needs more than that to leave within
    /// its own period (`network_pacing::video_frame_wire_rate_bps`'s rule). B5 compares the two.
    Paced { ceiling_bps: u64 },
}

#[derive(Clone, Copy, Debug)]
pub struct TransportOptions {
    pub pacing: Pacing,
    pub video_queue: usize,
    pub audio_queue: usize,
}

impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            pacing: Pacing::Paced {
                ceiling_bps: 800_000_000,
            },
            video_queue: DEFAULT_VIDEO_QUEUE,
            audio_queue: DEFAULT_AUDIO_QUEUE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdrReason {
    Pli,
    Fir,
    Connected,
    QueueFull,
    WaitingForKeyframe,
    WriteFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportEvent {
    Connected,
    Closed,
    /// A channel the host expects is open.
    ChannelOpen(String),
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
    pub capture: Instant,
}

/// What str0m wants on the wire and how.
#[derive(Debug)]
pub struct Outgoing {
    pub destination: SocketAddr,
    pub data: Vec<u8>,
    /// Media RTP, which the pacer spreads. STUN, DTLS, SCTP and RTCP go out at once.
    pub paced: bool,
}

/// RTP, not RTCP, STUN or DTLS (RFC 7983 demultiplexing, rtcp-mux).
fn is_media_rtp(data: &[u8]) -> bool {
    data.len() > 12 && (128..192).contains(&data[0]) && !(192..224).contains(&data[1])
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
    frame_period: Duration,
    authenticated: Arc<AtomicBool>,
    input: Option<ChannelId>,
    control_open: bool,
    gamepad_open: bool,
    outgoing: VecDeque<Outgoing>,
    events: Vec<TransportEvent>,
    idr: Vec<IdrReason>,
    connected: bool,
    closed: bool,
    /// Wire rate one frame needs to leave within its period, for the pacer.
    rate_floor_bps: u64,
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
            frame_period: Duration::from_secs_f64(1. / f64::from(spec.video.fps.max(1))),
            authenticated,
            input: None,
            control_open: false,
            gamepad_open: false,
            outgoing: VecDeque::new(),
            events: Vec::new(),
            idr: Vec::new(),
            connected: false,
            closed: false,
            rate_floor_bps: 0,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected && self.rtc.is_alive()
    }

    pub fn is_closed(&self) -> bool {
        self.closed || !self.rtc.is_alive()
    }

    /// The wire rate the latest frame needs, in bits per second.
    pub fn rate_floor_bps(&self) -> u64 {
        self.rate_floor_bps
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

    /// A datagram that arrived on the socket. Garbage (not STUN, DTLS, RTP or RTCP) is dropped.
    pub fn handle_datagram(&mut self, now: Instant, from: SocketAddr, data: &[u8]) -> Result<()> {
        let receive = match Receive::new(Protocol::Udp, from, self.local, data) {
            Ok(receive) => receive,
            Err(e) => {
                tracing::debug!("webrtc: unparsable datagram from {from}: {e}");
                return Ok(());
            }
        };
        self.rtc
            .handle_input(Input::Receive(now, receive))
            .map_err(|e| anyhow!("webrtc transport failed: {e}"))
    }

    /// Advance the clock and collect everything str0m has ready. Returns when it wants to be
    /// called again.
    pub fn advance(&mut self, now: Instant) -> Result<Instant> {
        self.rtc
            .handle_input(Input::Timeout(now))
            .map_err(|e| anyhow!("webrtc transport failed: {e}"))?;
        self.poll(now)
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
                    let paced = is_media_rtp(&data);
                    self.outgoing.push_back(Outgoing {
                        destination: t.destination,
                        data,
                        paced,
                    });
                }
                Output::Event(event) => self.on_event(event),
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Connected => {
                self.connected = true;
                self.events.push(TransportEvent::Connected);
                // Nothing was sent before this point, so the picture starts with a keyframe.
                self.idr.push(IdrReason::Connected);
            }
            Event::IceConnectionStateChange(state) => {
                tracing::debug!("webrtc: ICE {state:?}");
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
                if self.input == Some(id) {
                    self.input = None;
                }
            }
            Event::Closed => self.closed = true,
            _ => {}
        }
    }

    fn on_channel_open(&mut self, id: ChannelId, label: String) {
        match label.as_str() {
            CONTROL_LABEL if id == self.ids.control => {
                self.control_open = true;
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            GAMEPAD_LABEL if id == self.ids.gamepad => {
                self.gamepad_open = true;
                self.events.push(TransportEvent::ChannelOpen(label));
            }
            INPUT_LABEL if self.input.is_none() && self.authenticated.load(Ordering::Acquire) => {
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
        let packets = frame.data.len().div_ceil(RTP_PAYLOAD_TARGET).max(1) as f64;
        self.rate_floor_bps = (packets
            * (DATAGRAM_TARGET + network_pacing::overhead(self.local.is_ipv6())) as f64
            * 8.
            / self.frame_period.as_secs_f64()) as u64;
        Ok(true)
    }

    /// One Opus packet. Stereo and multistream payloads are not looked at.
    pub fn write_audio(&mut self, now: Instant, packet: &AudioPacket) -> Result<bool> {
        if !self.is_connected() {
            return Ok(false);
        }
        let time = MediaTime::new(
            ticks(packet.capture.saturating_duration_since(self.start), 48_000),
            Frequency::FORTY_EIGHT_KHZ,
        );
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

/// Paces the media packets and passes everything else straight through.
pub struct SendQueue {
    pacing: Pacing,
    pacer: Pacer,
    ipv6: bool,
    floor_bps: u64,
    immediate: VecDeque<Outgoing>,
    paced: VecDeque<Outgoing>,
}

#[derive(Default)]
pub struct Batch {
    pub packets: Vec<Outgoing>,
    /// Bytes of the paced packets, for [`SendQueue::sent`].
    pub paced_bytes: usize,
    pub paced_packets: usize,
}

impl SendQueue {
    pub fn new(pacing: Pacing, ipv6: bool, now: Instant) -> Self {
        Self {
            pacing,
            pacer: Pacer::new(now),
            ipv6,
            floor_bps: 0,
            immediate: VecDeque::new(),
            paced: VecDeque::new(),
        }
    }

    pub fn set_rate_floor(&mut self, bps: u64) {
        self.floor_bps = bps;
    }

    pub fn push(&mut self, out: Outgoing) {
        if out.paced && self.pacing != Pacing::Off {
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
            Some(self.pacer.due())
        } else {
            None
        }
    }

    fn rate(&self) -> u64 {
        match self.pacing {
            Pacing::Off => u64::MAX,
            Pacing::Paced { ceiling_bps } => ceiling_bps.max(self.floor_bps).max(1),
        }
    }

    /// Everything unpaced, plus (when due) up to one millisecond of media at the pacing rate and
    /// always at least one datagram.
    pub fn next_batch(&mut self, now: Instant) -> Batch {
        let mut batch = Batch {
            packets: self.immediate.drain(..).collect(),
            ..Batch::default()
        };
        if now >= self.pacer.due() {
            let datagram = self.paced.front().map_or(0, |p| p.data.len());
            let limit = network_pacing::batch_payload_limit(
                self.rate(),
                datagram.max(1),
                self.ipv6,
                usize::MAX,
            );
            while let Some(front) = self.paced.front() {
                if batch.paced_packets > 0 && batch.paced_bytes + front.data.len() > limit {
                    break;
                }
                batch.paced_bytes += front.data.len();
                batch.paced_packets += 1;
                batch
                    .packets
                    .push(self.paced.pop_front().expect("front exists"));
            }
        }
        batch
    }

    /// Called right after the batch's datagrams were written to the socket.
    pub fn sent(&mut self, completed: Instant, batch: &Batch) {
        if batch.paced_packets > 0 {
            self.pacer.sent(
                completed,
                batch.paced_bytes,
                batch.paced_packets,
                self.ipv6,
                self.rate(),
            );
        }
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
}

struct Shared {
    stop: AtomicBool,
    authenticated: Arc<AtomicBool>,
    pending_video: AtomicUsize,
    pending_audio: AtomicUsize,
    /// A frame was dropped, so the decoder's reference is gone: send nothing but a keyframe.
    need_keyframe: AtomicBool,
    last_idr_ms: AtomicU64,
    epoch: Instant,
    counters: Counters,
    hooks: Hooks,
    options: TransportOptions,
}

impl Shared {
    fn request_idr(&self, reason: IdrReason) {
        self.counters.idr_requests.fetch_add(1, Ordering::Relaxed);
        (self.hooks.idr)(reason);
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
            authenticated: authenticated.clone(),
            pending_video: AtomicUsize::new(0),
            pending_audio: AtomicUsize::new(0),
            need_keyframe: AtomicBool::new(false),
            last_idr_ms: AtomicU64::new(0),
            epoch,
            counters: Counters::default(),
            hooks,
            options,
        });
        let (tx, rx) = mpsc::channel::<Command>();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .context("setting the receive timeout")?;
        let recv_socket = socket.try_clone().context("cloning the media socket")?;
        let engine = Engine::new(rtc, ids, spec, local, epoch, authenticated);
        let mut threads = Vec::new();
        let (shared_rx, tx_rx) = (shared.clone(), tx.clone());
        threads.push(
            std::thread::Builder::new()
                .name("webrtc-recv".into())
                .spawn(move || receive_loop(recv_socket, tx_rx, shared_rx))
                .context("starting the receive thread")?,
        );
        let shared_media = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("webrtc-media".into())
                .spawn(move || media_loop(engine, socket, rx, shared_media))
                .context("starting the media thread")?,
        );
        Ok(Self {
            tx,
            shared,
            threads,
            local,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
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
        let keyframe = frame.keyframe;
        if self.tx.send(Command::Video(frame)).is_err() {
            shared.pending_video.fetch_sub(1, Ordering::AcqRel);
            shared
                .counters
                .video_dropped
                .fetch_add(1, Ordering::Relaxed);
            return Submit::Dropped;
        }
        if keyframe {
            shared.need_keyframe.store(false, Ordering::Release);
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
        let shared = &self.shared;
        if shared.pending_audio.fetch_add(1, Ordering::AcqRel) >= shared.options.audio_queue
            || self.tx.send(Command::Audio(packet)).is_err()
        {
            shared.pending_audio.fetch_sub(1, Ordering::AcqRel);
            shared
                .counters
                .audio_dropped
                .fetch_add(1, Ordering::Relaxed);
            return Submit::Dropped;
        }
        Submit::Queued
    }

    pub fn send_control(&self, data: Vec<u8>, binary: bool) {
        let _ = self.tx.send(Command::Control { data, binary });
    }

    pub fn send_gamepad(&self, data: Vec<u8>, binary: bool) {
        let _ = self.tx.send(Command::Gamepad { data, binary });
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
        }
    }

    pub fn close(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.tx.send(Command::Stop);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        self.close();
    }
}

fn receive_loop(socket: UdpSocket, tx: Sender<Command>, shared: Arc<Shared>) {
    let mut buf = vec![0u8; 2048];
    while !shared.stop.load(Ordering::Acquire) {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let command = Command::Datagram {
                    at: Instant::now(),
                    from,
                    data: buf[..len].to_vec(),
                };
                if tx.send(command).is_err() {
                    return;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            // Windows reports an ICMP port-unreachable for an earlier send as a receive error.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => {
                tracing::error!("webrtc: media socket receive failed: {e}");
                let _ = tx.send(Command::Stop);
                return;
            }
        }
    }
}

fn media_loop(
    mut engine: Engine,
    socket: UdpSocket,
    rx: mpsc::Receiver<Command>,
    shared: Arc<Shared>,
) {
    let ipv6 = socket.local_addr().is_ok_and(|a| a.is_ipv6());
    let mut queue = SendQueue::new(shared.options.pacing, ipv6, Instant::now());
    let result = (|| -> Result<()> {
        loop {
            let now = Instant::now();
            let timeout = engine.advance(now)?;
            for out in engine.take_outgoing() {
                queue.push(out);
            }
            queue.set_rate_floor(engine.rate_floor_bps());
            dispatch(&mut engine, &shared);
            if engine.is_closed() {
                return Ok(());
            }
            send_due(&socket, &mut queue, &shared);
            let now = Instant::now();
            let wake = queue.due(now).map_or(timeout, |due| due.min(timeout));
            let command = match wake.checked_duration_since(now).filter(|d| !d.is_zero()) {
                Some(wait) => rx.recv_timeout(wait),
                None => rx.try_recv().map_err(|e| match e {
                    mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                    mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
                }),
            };
            match command {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(command) => handle_command(&mut engine, &shared, command)?,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    })();
    if let Err(e) = result {
        tracing::error!("webrtc: media thread stopped: {e:#}");
    }
    engine.disconnect();
    (shared.hooks.event)(TransportEvent::Closed);
}

fn handle_command(engine: &mut Engine, shared: &Shared, command: Command) -> Result<()> {
    let now = Instant::now();
    match command {
        Command::Datagram { at, from, data } => engine.handle_datagram(at.min(now), from, &data)?,
        Command::Video(frame) => {
            shared.pending_video.fetch_sub(1, Ordering::AcqRel);
            match engine.write_video(now, &frame) {
                Ok(true) => {
                    shared.counters.video_frames.fetch_add(1, Ordering::Relaxed);
                    shared
                        .counters
                        .video_bytes
                        .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                }
                Ok(false) => {
                    shared
                        .counters
                        .video_dropped
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::error!("webrtc: {e:#}");
                    shared
                        .counters
                        .video_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    shared.need_keyframe.store(true, Ordering::Release);
                    shared.request_idr(IdrReason::WriteFailed);
                }
            }
        }
        Command::Audio(packet) => {
            shared.pending_audio.fetch_sub(1, Ordering::AcqRel);
            match engine.write_audio(now, &packet) {
                Ok(true) => {
                    shared
                        .counters
                        .audio_packets
                        .fetch_add(1, Ordering::Relaxed);
                }
                Ok(false) => {
                    shared
                        .counters
                        .audio_dropped
                        .fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::error!("webrtc: {e:#}");
                    shared
                        .counters
                        .audio_dropped
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Command::Control { data, binary } => {
            if let Err(e) = engine.send_control(&data, binary) {
                tracing::warn!("webrtc: control message not sent: {e:#}");
            }
        }
        Command::Gamepad { data, binary } => {
            if let Err(e) = engine.send_gamepad(&data, binary) {
                tracing::warn!("webrtc: gamepad message not sent: {e:#}");
            }
        }
        Command::Stop => {}
    }
    Ok(())
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
    while queue.due(now).is_some_and(|due| due <= now) {
        let batch = queue.next_batch(now);
        if batch.packets.is_empty() {
            break;
        }
        for out in &batch.packets {
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
    use super::super::sdp::testing::{answer, client, host_offer, spec, surround_spec};
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

    /// The client's answer for a surround offer: str0m's own client only knows stereo Opus, so it
    /// answers that reading and the multiopus lines are put back the way Chromium would send them.
    fn multiopus_answer(spec: &OfferSpec, offer: &str, client: &mut Rtc) -> String {
        let Some(layout) = spec.audio.layout() else {
            return answer(client, offer);
        };
        let reading = offer.replace(
            &format!("multiopus/48000/{}", layout.channels),
            "opus/48000/2",
        );
        let mapping: Vec<String> = layout.mapping.iter().map(u8::to_string).collect();
        let fmtp = format!(
            "a=fmtp:111 channel_mapping={};num_streams={};coupled_streams={};minptime=10;useinbandfec=0",
            mapping.join(","),
            layout.streams,
            layout.coupled
        );
        let mut text = answer(client, &reading)
            .replace(
                "a=rtpmap:111 opus/48000/2",
                &format!("a=rtpmap:111 multiopus/48000/{}", layout.channels),
            )
            .lines()
            .map(|l| {
                if l.starts_with("a=fmtp:111 ") {
                    fmtp.clone()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n");
        text.push_str("\r\n");
        text
    }

    impl Link {
        fn new(spec: OfferSpec) -> Self {
            let host_addr: SocketAddr = HOST.parse().unwrap();
            let (mut rtc, offer) = host_offer(&spec, host_addr);
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
                            self.host
                                .handle_datagram(self.now, client_addr, &data)
                                .unwrap();
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

        fn audio(&mut self, data: Vec<u8>, at_ms: u64) {
            let packet = AudioPacket {
                data,
                capture: self.at(at_ms),
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
                link.audio(p.clone(), 10 * i as u64);
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

    #[test]
    fn the_pacer_sends_one_millisecond_of_media_per_group() {
        let now = Instant::now();
        // 12 Mbps is 1500 bytes of wire traffic a millisecond: one 1100-byte datagram (1166 on the
        // wire) fits, a second does not.
        let mut queue = SendQueue::new(
            Pacing::Paced {
                ceiling_bps: 12_000_000,
            },
            false,
            now,
        );
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
    fn a_large_frame_raises_the_pacing_rate_to_what_it_needs() {
        let now = Instant::now();
        let mut queue = SendQueue::new(
            Pacing::Paced {
                ceiling_bps: 12_000_000,
            },
            false,
            now,
        );
        queue.set_rate_floor(120_000_000);
        for _ in 0..40 {
            queue.push(rtp_packet(1100));
        }
        let batch = queue.next_batch(now);
        assert!(batch.paced_packets >= 10, "{}", batch.paced_packets);
    }

    #[test]
    fn pacing_off_hands_everything_over_at_once() {
        let now = Instant::now();
        let mut queue = SendQueue::new(Pacing::Off, false, now);
        for _ in 0..50 {
            queue.push(rtp_packet(1100));
        }
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
            authenticated: Arc::new(AtomicBool::new(false)),
            pending_video: AtomicUsize::new(0),
            pending_audio: AtomicUsize::new(0),
            need_keyframe: AtomicBool::new(false),
            last_idr_ms: AtomicU64::new(0),
            epoch: Instant::now(),
            counters: Counters::default(),
            hooks,
            options,
        });
        (
            MediaHandle {
                tx,
                shared,
                threads: Vec::new(),
                local: HOST.parse().unwrap(),
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
