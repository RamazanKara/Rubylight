//! The streaming session: the Rust counterpart of moonlight-common-c's LiStartConnection and
//! LiStopConnection. [`Session::start`] resolves the host, runs the RTSP handshake, connects
//! the control stream and starts the video, audio and input paths on their own threads;
//! [`Session::stop`] (or dropping the session) tears them down in the C's order.
//!
//! Ported from moonlight-common-c (GPL-3.0, Cameron Gutman and contributors):
//! `Connection.c`, the thread and socket parts of `VideoStream.c`, `AudioStream.c` and
//! `ControlStream.c`, with Nightjar's `android_connection.c` (input-only sessions, the
//! microphone with the last stage), its network grace and video receive buffer patches.
//!
//! Applications plug in a [`VideoDecoder`], an [`AudioRenderer`] and a [`Listener`]; the
//! session never decodes, renders or plays anything itself. It is platform-neutral: the
//! Android JNI bridge and the desktop clients use the same API.

mod audio;
mod net;
mod video;

use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub use audio::AUDIO_QUEUE_BOUND;
pub use net::{is_nat64_with, is_private};
pub use video::DECODE_UNIT_QUEUE_BOUND;

use crate::audio::{AudioConfig, AudioEncryption, AudioReceiver, AudioStats};
use crate::control::driver::{self, ControlHandle, DriverConfig};
use crate::control::{
    ClientMessage, ConnectionStatus, ControlEvent, HdrMetadata, HostCapabilities, SendError,
    SessionOptions,
};
use crate::input::microphone;
use crate::input::InputQueue;
use crate::priority::{self, Qos};
use crate::rtsp::{self, Handshake, HandshakeParams, OpusConfig, RtspError, RtspOutcome};
use crate::sdp::{self, AudioConfiguration, StreamConfig, StreamLocation};
use crate::video::{Frame, VideoConfig, VideoReceiver, VideoStats};

/// Microseconds on the session's monotonic clock (`LiGetMicroseconds`). Frame receive and
/// enqueue times use it.
pub fn now_us() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// Termination codes the session itself reports, besides the host's
/// ([`crate::control::events::termination`]) and socket errors.
pub mod error {
    pub use crate::control::events::termination::*;
    /// No video packet arrived within 10 seconds (`ML_ERROR_NO_VIDEO_TRAFFIC`).
    pub const NO_VIDEO_TRAFFIC: i32 = -100;
    /// Video arrived but no complete key frame within 10 seconds (`ML_ERROR_NO_VIDEO_FRAME`).
    pub const NO_VIDEO_FRAME: i32 = -101;
    /// Packed 4:4:4 cannot be streamed with this host ([`crate::rtsp::RtspError::PackedRefused`],
    /// Java's `CLIENT_ERROR_PACKED_444_REFUSED`): the client may reconnect without it.
    pub const PACKED_444_REFUSED: i32 = -106;
    /// A failure without an OS error code.
    pub const FAILED: i32 = -1;
}

/// Connection stages (`STAGE_*`), in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    PlatformInit = 1,
    NameResolution = 2,
    AudioStreamInit = 3,
    RtspHandshake = 4,
    ControlStreamInit = 5,
    VideoStreamInit = 6,
    InputStreamInit = 7,
    ControlStreamStart = 8,
    VideoStreamStart = 9,
    AudioStreamStart = 10,
    InputStreamStart = 11,
}

impl Stage {
    /// `LiGetStageName`.
    pub fn name(self) -> &'static str {
        match self {
            Stage::PlatformInit => "platform initialization",
            Stage::NameResolution => "name resolution",
            Stage::AudioStreamInit => "audio stream initialization",
            Stage::RtspHandshake => "RTSP handshake",
            Stage::ControlStreamInit => "control stream initialization",
            Stage::VideoStreamInit => "video stream initialization",
            Stage::InputStreamInit => "input stream initialization",
            Stage::ControlStreamStart => "control stream establishment",
            Stage::VideoStreamStart => "video stream establishment",
            Stage::AudioStreamStart => "audio stream establishment",
            Stage::InputStreamStart => "input stream establishment",
        }
    }
}

/// What the session needs to know about the host (`SERVER_INFORMATION`).
#[derive(Clone, Debug)]
pub struct HostInfo {
    /// Host name or address, as given to the HTTP client.
    pub address: String,
    /// serverinfo `appversion`, e.g. "7.1.431.-1".
    pub app_version: String,
    /// `sessionUrl0` from the launch or resume reply.
    pub rtsp_session_url: Option<String>,
    /// ServerCodecModeSupport, trusted (see `ServerInfo::trusted_codec_mode_support`).
    pub server_codec_mode_support: u32,
}

/// Where the host is: `STREAM_CFG_LOCAL`, `STREAM_CFG_REMOTE` or `STREAM_CFG_AUTO`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Location {
    Local,
    Remote,
    /// Local for a private address that is not NAT64-synthesized; otherwise remote, with
    /// the video packet size set to 1024 (IPv4, NAT64) or 1184 (IPv6).
    #[default]
    Auto,
}

#[derive(Clone, Debug, Default)]
pub struct SessionConfig {
    /// The stream; its `location` is replaced by [`SessionConfig::location`]'s decision.
    pub stream: StreamConfig,
    pub location: Location,
    /// Nightjar's network grace period (`LiSetNetworkGracePeriodMs`); see
    /// [`DriverConfig::network_grace`].
    pub network_grace: Duration,
    /// Control and input only: no video or audio threads, decoders or timeouts
    /// (Nightjar's input-only sessions).
    pub input_only: bool,
    /// A remote monitor session (Nightjar's `AndroidRemoteMonitor`): view-only. The input
    /// stream is never started, so nothing is sent from this client, and the mouse is not
    /// wiggled to wake the host's cursor.
    pub remote_monitor: bool,
    /// Send mouse and pen motion at once instead of batching it for 1 ms (Nightjar's
    /// "Unbatched input").
    pub unbatched_input: bool,
    /// Nightjar's "Prioritize stream traffic": `SO_PRIORITY` 6 on every socket of the stream
    /// and nice -4 on the receive, decode and control threads. See [`crate::priority`].
    pub network_priority: bool,
}

/// The decoder's answer for a frame (`DR_OK`, `DR_NEED_IDR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeStatus {
    Ok,
    NeedIdr,
}

/// The negotiated video stream, for [`VideoDecoder::setup`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoSetup {
    /// `VIDEO_FORMAT_*`.
    pub format: u32,
    pub width: i32,
    pub height: i32,
    pub fps: i32,
    pub chroma_packing: u8,
    pub logical_width: i32,
    pub logical_height: i32,
    pub color_space: i32,
    pub color_range: i32,
}

/// The video decoder (`DECODER_RENDERER_CALLBACKS`). Frames come from the decoder thread, or
/// from the receive thread when the stream's video capabilities include
/// `CAPABILITY_DIRECT_SUBMIT` (cleared for PyroWave). `stop` and `cleanup` run after those
/// threads have ended.
pub trait VideoDecoder: Send {
    /// Before any frame. An error fails the video stage with its code.
    fn setup(&mut self, setup: &VideoSetup) -> Result<(), i32>;
    fn start(&mut self) {}
    fn submit(&mut self, frame: &Frame) -> DecodeStatus;
    /// The decoder wants the packets of a PyroWave records frame while it arrives
    /// ([`VideoDecoder::chunk`]). Only asked once, before the stream starts, and honoured on the
    /// decoder thread only.
    fn wants_early_chunks(&self) -> bool {
        false
    }
    /// One packet of the records envelope of the next [`VideoDecoder::submit`] frame; see
    /// [`crate::video::VideoSink::chunk`]. Chunks the decoder never sees (a full queue) are
    /// caught by the frame itself, which carries them all.
    fn chunk(&mut self, _first: bool, _flags: u32, _payload: &[u8]) {}
    fn stop(&mut self) {}
    fn cleanup(&mut self) {}
}

/// The negotiated audio stream, for [`AudioRenderer::init`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSetup {
    pub configuration: AudioConfiguration,
    /// The high-quality configuration when high-quality surround is on, else the normal one.
    pub opus: OpusConfig,
    pub packet_duration_ms: u32,
    /// 48 samples per millisecond of packet.
    pub samples_per_frame: u32,
}

/// The audio decoder and output (`AUDIO_RENDERER_CALLBACKS`). Packets come from the audio
/// decoder thread, or from the receive thread with `CAPABILITY_DIRECT_SUBMIT` in the
/// stream's audio capabilities.
pub trait AudioRenderer: Send {
    fn init(&mut self, setup: &AudioSetup) -> Result<(), i32>;
    fn start(&mut self) {}
    /// An Opus packet in order, or `None` for a lost one (run packet loss concealment).
    fn play(&mut self, packet: Option<&[u8]>);
    fn stop(&mut self) {}
    fn cleanup(&mut self) {}
}

/// Connection callbacks (`CONNECTION_LISTENER_CALLBACKS`), called from session threads.
pub trait Listener: Send + Sync {
    fn stage_starting(&self, _stage: Stage) {}
    fn stage_complete(&self, _stage: Stage) {}
    fn stage_failed(&self, _stage: Stage, _error: i32) {}
    fn connection_started(&self) {}
    /// The control message types the host announced, as soon as the handshake has them: before
    /// the decoder and audio renderer start, so their `start` can already ask what the host
    /// supports (the session is not returned until every stage has run).
    fn host_control_messages(&self, _messages: &[u16]) {}
    /// The stream ended without [`Session::stop`]: the host's reason, a control stream
    /// failure, a video timeout or a socket error. Called once, on its own thread, so it may
    /// stop the session.
    fn connection_terminated(&self, _error: i32) {}
    /// Everything else the host sends or the control stream notices: connection status,
    /// rumble, trigger rumble, motion requests, LEDs, adaptive triggers, HDR mode, controller
    /// haptics and audio, host timing, clock sync replies and cursor shapes. Called on the
    /// control thread.
    fn control_event(&self, _event: ControlEvent) {}
    fn log(&self, _message: &str) {}
    /// Something that does not stop the stream but should be seen: a socket option or thread
    /// priority the OS refused. Logged like [`Listener::log`] unless overridden.
    fn warn(&self, message: &str) {
        self.log(message);
    }
}

/// Aborts a [`Session::start`] in progress from another thread (`LiInterruptConnection`).
#[derive(Clone, Debug, Default)]
pub struct Interrupt(Arc<AtomicBool>);

impl Interrupt {
    pub fn interrupt(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_interrupted(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why [`Session::start`] failed: the stage and its error code, as `stageFailed` reports.
#[derive(Debug)]
pub struct StartError {
    pub stage: Stage,
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} failed ({}): {}",
            self.stage.name(),
            self.code,
            self.message
        )
    }
}

impl std::error::Error for StartError {}

/// Counters and measurements of a running session.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionStats {
    /// ENet's round-trip time and its variance.
    pub rtt: Option<(Duration, Duration)>,
    pub video: VideoStats,
    pub audio: AudioStats,
    /// Frames given to the decoder.
    pub frames_submitted: u64,
    /// Frames the decoder answered with [`DecodeStatus::NeedIdr`].
    pub frames_need_idr: u64,
    /// Times the 15-frame decode queue overflowed and was flushed.
    pub decode_queue_overflows: u64,
    /// Early chunks dropped because the decoder fell behind (their frames decode whole).
    pub early_chunks_dropped: u64,
    pub decode_queue_len: usize,
    /// Opus packets and loss markers delivered, and the pending queue.
    pub audio_packets: u64,
    pub audio_queue_len: usize,
    pub audio_queue_overflows: u64,
    pub connection_status: Option<ConnectionStatus>,
    /// Input held back during a network grace period.
    pub input_contact_lost: bool,
}

/// State every session thread shares.
pub(crate) struct Shared {
    listener: Arc<dyn Listener>,
    stopping: AtomicBool,
    terminated: AtomicBool,
    /// "Prioritize stream traffic" ([`SessionConfig::network_priority`]).
    network_priority: bool,
}

impl Shared {
    /// Reports a termination once, on its own thread, unless the session is stopping.
    pub(crate) fn terminate(&self, code: i32) {
        if self.stopping.load(Ordering::SeqCst) || self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        let listener = self.listener.clone();
        let _ = std::thread::Builder::new()
            .name("AsyncTerm".into())
            .spawn(move || listener.connection_terminated(code));
    }

    pub(crate) fn log(&self, message: &str) {
        self.listener.log(message);
    }

    pub(crate) fn warn(&self, message: &str) {
        self.listener.warn(message);
    }

    /// Raises the calling session thread when "prioritize stream traffic" is on, and says so
    /// in the log if the OS refuses.
    pub(crate) fn prioritize_thread(&self) {
        if let Err(e) = priority::prioritize_current_thread(self.network_priority) {
            self.warn(&format!(
                "Could not raise the priority of {}: {e}",
                std::thread::current().name().unwrap_or("a stream thread")
            ));
        }
    }
}

struct Microphone {
    socket: UdpSocket,
    target: SocketAddr,
    key: [u8; 16],
    key_id: u32,
    sequence: u16,
    started: Instant,
}

/// Everything a session owns; filled stage by stage so a failed start tears down exactly
/// what it built.
struct Parts {
    shared: Arc<Shared>,
    remote: Option<SocketAddr>,
    outcome: Option<RtspOutcome>,
    audio_socket: Option<UdpSocket>,
    audio_pinger: Option<net::Pinger>,
    /// An input-only session's video pings, with no receive side behind them.
    video_pinger: Option<net::Pinger>,
    video_shared: Option<Arc<video::VideoShared>>,
    audio_shared: Arc<audio::AudioShared>,
    control: Option<ControlHandle>,
    control_thread: Option<JoinHandle<driver::Outcome>>,
    video: Option<video::VideoStream>,
    video_decoder: Option<Box<dyn VideoDecoder>>,
    audio: Option<audio::AudioStream>,
    audio_renderer: Option<Box<dyn AudioRenderer>>,
    microphone: Mutex<Option<Microphone>>,
}

impl Parts {
    /// LiStopConnection's order: input, audio, video, control, then the audio socket.
    fn teardown(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        *self.microphone.lock().unwrap_or_else(|p| p.into_inner()) = None;
        if let Some(stream) = self.audio.take() {
            if let Some(renderer) = stream.stop() {
                self.audio_renderer = Some(renderer);
            }
        }
        if let Some(mut renderer) = self.audio_renderer.take() {
            renderer.stop();
            renderer.cleanup();
        }
        if let Some(stream) = self.video.take() {
            if let Some(decoder) = stream.stop() {
                self.video_decoder = Some(decoder);
            }
        }
        if let Some(mut pinger) = self.video_pinger.take() {
            pinger.stop();
        }
        if let Some(mut decoder) = self.video_decoder.take() {
            decoder.stop();
            decoder.cleanup();
        }
        if let Some(control) = &self.control {
            control.stop();
        }
        if let Some(thread) = self.control_thread.take() {
            let _ = thread.join();
        }
        if let Some(mut pinger) = self.audio_pinger.take() {
            pinger.stop();
        }
        self.audio_socket = None;
    }
}

fn rtsp_error_code(error: &RtspError) -> i32 {
    match error {
        RtspError::Status { code, .. } => *code,
        RtspError::Io(e) => e.raw_os_error().unwrap_or(error::FAILED),
        RtspError::PackedRefused(_) => error::PACKED_444_REFUSED,
        _ => error::FAILED,
    }
}

fn io_code(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(error::FAILED)
}

/// A running stream. Dropping it stops it.
pub struct Session {
    parts: Parts,
}

impl Session {
    /// Connects and starts streaming; blocks until the stream runs or a stage fails. Every
    /// stage is reported to `listener` as it starts, completes or fails.
    pub fn start(
        host: &HostInfo,
        config: SessionConfig,
        video: Box<dyn VideoDecoder>,
        audio: Box<dyn AudioRenderer>,
        listener: Arc<dyn Listener>,
        interrupt: &Interrupt,
    ) -> Result<Session, StartError> {
        let mut parts = Parts {
            shared: Arc::new(Shared {
                listener: listener.clone(),
                stopping: AtomicBool::new(false),
                terminated: AtomicBool::new(false),
                network_priority: config.network_priority,
            }),
            remote: None,
            outcome: None,
            audio_socket: None,
            audio_pinger: None,
            video_pinger: None,
            video_shared: None,
            audio_shared: Arc::new(audio::AudioShared::default()),
            control: None,
            control_thread: None,
            video: None,
            video_decoder: None,
            audio: None,
            audio_renderer: None,
            microphone: Mutex::new(None),
        };
        match start(
            &mut parts,
            host,
            config,
            video,
            audio,
            listener.as_ref(),
            interrupt,
        ) {
            Ok(()) => Ok(Session { parts }),
            Err(error) => {
                listener.stage_failed(error.stage, error.code);
                parts.teardown();
                Err(error)
            }
        }
    }

    fn control(&self) -> &ControlHandle {
        self.parts
            .control
            .as_ref()
            .expect("a started session has a control stream")
    }

    /// The negotiated stream: format, ports, encryption, host control messages and more.
    pub fn outcome(&self) -> &RtspOutcome {
        self.parts
            .outcome
            .as_ref()
            .expect("a started session has negotiated")
    }

    /// The host's resolved address.
    pub fn remote_address(&self) -> SocketAddr {
        self.parts
            .remote
            .expect("a started session has resolved the host")
    }

    /// Runs `f` on the input queue (`LiSend*`): keyboard, mouse, controllers, touch, pen,
    /// motion and battery events.
    pub fn input<R>(&self, f: impl FnOnce(&mut InputQueue) -> R) -> R {
        self.control().with_input(f)
    }

    /// `LiRequestIdrFrame`: drops pending invalidations and asks for a key frame.
    pub fn request_idr_frame(&self) {
        self.control().with_session(|s| s.request_idr_frame());
    }

    /// Sends a control message, such as Rubylight's 0x55xx messages (phase lock reports,
    /// display caps, reconfigure, host timing requests, cursor shape requests, controller
    /// audio).
    pub fn send(&self, message: ClientMessage) -> Result<(), SendError> {
        self.control().send(message)
    }

    /// Sends a clock sync probe (0x5537) stamped `client_us` and returns its sequence.
    pub fn send_clock_sync_probe(&self, client_us: u64) -> Result<u32, SendError> {
        self.control()
            .with_session(|s| s.send_clock_sync_probe(client_us))
    }

    /// Runs `f` on the control session itself, for anything not covered above.
    pub fn with_control<R>(&self, f: impl FnOnce(&mut crate::control::ControlSession) -> R) -> R {
        self.control().with_session(f)
    }

    /// The host display's HDR state and metadata (`LiGetHdrMetadata`).
    pub fn hdr(&self) -> (bool, Option<HdrMetadata>) {
        self.control()
            .with_session_quiet(|s| (s.hdr_enabled(), s.hdr_metadata()))
    }

    /// Whether a microphone stream was negotiated with encryption.
    pub fn microphone_available(&self) -> bool {
        self.parts
            .microphone
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// Sends one encoded mono Opus packet on the microphone stream.
    pub fn send_microphone(&self, opus: &[u8]) -> io::Result<()> {
        let mut guard = self
            .parts
            .microphone
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mic = guard
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "no microphone stream"))?;
        let timestamp = mic.started.elapsed().as_millis() as u32;
        let packet = microphone::build_packet(&mic.key, mic.key_id, mic.sequence, timestamp, opus)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad Opus packet size"))?;
        mic.sequence = mic.sequence.wrapping_add(1);
        mic.socket.send_to(&packet, mic.target).map(|_| ())
    }

    pub fn stats(&self) -> SessionStats {
        let control = self.control();
        let mut stats = SessionStats {
            rtt: control.round_trip_time(),
            input_contact_lost: control.contact_lost(),
            connection_status: Some(control.with_session_quiet(|s| s.connection_status())),
            audio: *self
                .parts
                .audio_shared
                .stats
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            audio_packets: self.parts.audio_shared.packets.load(Ordering::Relaxed),
            audio_queue_len: self.parts.audio_shared.queue.len(),
            audio_queue_overflows: self
                .parts
                .audio_shared
                .queue
                .overflows
                .load(Ordering::Relaxed),
            ..Default::default()
        };
        if let Some(video) = &self.parts.video_shared {
            stats.video = video
                .receiver
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .stats();
            stats.frames_submitted = video.counters.submitted.load(Ordering::Relaxed);
            stats.frames_need_idr = video.counters.need_idr.load(Ordering::Relaxed);
            stats.decode_queue_overflows = video.counters.queue_overflows.load(Ordering::Relaxed);
            stats.early_chunks_dropped = video.counters.chunks_dropped.load(Ordering::Relaxed);
            stats.decode_queue_len = video.queue.len();
        }
        stats
    }

    /// `LiStopConnection`: stops every stream; no termination is reported afterwards.
    pub fn stop(mut self) {
        self.parts.teardown();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.parts.teardown();
    }
}

/// The local/remote decision and the packet size cap for `Auto`.
fn decide_location(location: Location, remote: IpAddr, stream: &mut StreamConfig) {
    stream.location = match location {
        Location::Local => StreamLocation::Local,
        Location::Remote => StreamLocation::Remote,
        Location::Auto => {
            let nat64 = net::is_nat64(remote);
            if !nat64 && net::is_private(remote) {
                StreamLocation::Local
            } else {
                stream.packet_size = if remote.is_ipv4() || nat64 {
                    1024
                } else {
                    1184
                };
                StreamLocation::Remote
            }
        }
    };
}

fn start(
    parts: &mut Parts,
    host: &HostInfo,
    mut config: SessionConfig,
    video_decoder: Box<dyn VideoDecoder>,
    audio_renderer: Box<dyn AudioRenderer>,
    listener: &dyn Listener,
    interrupt: &Interrupt,
) -> Result<(), StartError> {
    let fail = |stage: Stage, code: i32, message: String| StartError {
        stage,
        code,
        message,
    };
    let check_interrupt = |stage: Stage| {
        if interrupt.is_interrupted() {
            Err(fail(stage, error::FAILED, "interrupted".into()))
        } else {
            Ok(())
        }
    };
    if host.server_codec_mode_support == 0 {
        return Err(fail(
            Stage::PlatformInit,
            error::FAILED,
            "server_codec_mode_support must be set".into(),
        ));
    }

    listener.stage_starting(Stage::PlatformInit);
    listener.stage_complete(Stage::PlatformInit);

    listener.stage_starting(Stage::NameResolution);
    let rtsp_port = rtsp::rtsp_port_from_session_url(host.rtsp_session_url.as_deref());
    // The well-known HTTP ports always listen on a standard host; 48010 may not yet.
    let test_ports: &[u16] = if rtsp_port != rtsp::DEFAULT_RTSP_PORT {
        &[rtsp_port]
    } else {
        &[47984, 47989, 48010]
    };
    let remote = net::resolve(&host.address, test_ports)
        .or_else(|_| {
            std::thread::sleep(Duration::from_secs(1));
            net::resolve(&host.address, test_ports)
        })
        .map_err(|e| fail(Stage::NameResolution, io_code(&e), e.to_string()))?;
    let local = net::local_address_toward(SocketAddr::new(remote.ip(), rtsp_port))
        .map_err(|e| fail(Stage::NameResolution, io_code(&e), e.to_string()))?;
    parts.remote = Some(remote);
    listener.stage_complete(Stage::NameResolution);
    decide_location(config.location, remote.ip(), &mut config.stream);

    listener.stage_starting(Stage::AudioStreamInit);
    listener.stage_complete(Stage::AudioStreamInit);

    listener.stage_starting(Stage::RtspHandshake);
    let scope = match remote {
        SocketAddr::V6(v6) => v6.scope_id(),
        SocketAddr::V4(_) => 0,
    };
    let rtsp_fail = |e: RtspError| fail(Stage::RtspHandshake, rtsp_error_code(&e), e.to_string());
    let mut handshake = Handshake::new(HandshakeParams {
        address: remote.ip(),
        ipv6_scope_id: scope,
        app_version: host.app_version.clone(),
        server_codec_mode_support: host.server_codec_mode_support,
        rtsp_session_url: host.rtsp_session_url.clone(),
        config: config.stream.clone(),
    })
    .map_err(rtsp_fail)?;
    let mut rtsp_address = remote;
    rtsp_address.set_port(handshake.rtsp_port());
    let interrupted = interrupt.0.clone();
    while let Some(request) = handshake.next_request().map_err(rtsp_fail)? {
        let response = rtsp::transact(
            rtsp_address,
            &request,
            &interrupted,
            config.network_priority,
            |m| listener.warn(m),
        )
        .map_err(rtsp_fail)?;
        handshake.handle_response(&response).map_err(rtsp_fail)?;
        if parts.audio_socket.is_none() {
            if let Some(port) = handshake.audio_port() {
                // notifyAudioPortNegotiationComplete: ping before PLAY.
                let socket = net::bind_udp(local, 0, Qos::Audio, config.network_priority, |m| {
                    listener.warn(m)
                })
                .map_err(|e| fail(Stage::RtspHandshake, io_code(&e), e.to_string()))?;
                parts.audio_pinger = Some(
                    net::Pinger::start(
                        "AudioPing",
                        &socket,
                        SocketAddr::new(remote.ip(), port),
                        handshake.audio_ping_payload(),
                    )
                    .map_err(|e| fail(Stage::RtspHandshake, io_code(&e), e.to_string()))?,
                );
                parts.audio_socket = Some(socket);
            }
        }
    }
    let outcome = handshake.finish().map_err(rtsp_fail)?;
    if outcome.packed_ten_bit_dropped() {
        listener.warn("Packed 4:4:4: no 10-bit mode both endpoints offer; streaming 8-bit SDR");
    }
    listener.host_control_messages(&outcome.host_control_messages);
    listener.stage_complete(Stage::RtspHandshake);
    check_interrupt(Stage::RtspHandshake)?;

    listener.stage_starting(Stage::ControlStreamInit);
    let caps = HostCapabilities {
        app_version: outcome.app_version,
        feature_flags: outcome.sunshine_feature_flags,
        encryption_enabled: outcome.encryption_enabled,
        control_messages: outcome.host_control_messages.clone(),
    };
    if !caps.encrypted_control() {
        return Err(fail(
            Stage::ControlStreamInit,
            error::FAILED,
            "the host's control stream generation is not supported (GFE before 3.22)".into(),
        ));
    }
    let mut driver_config = DriverConfig::new(SocketAddr::new(remote.ip(), outcome.control_port));
    if let SocketAddr::V6(v6) = &mut driver_config.remote {
        v6.set_scope_id(scope);
    }
    driver_config.local = Some(SocketAddr::new(local, 0));
    driver_config.connect_data = outcome.control_connect_data;
    driver_config.network_grace = config.network_grace;
    driver_config.unbatched_input = config.unbatched_input;
    driver_config.input_disabled = config.remote_monitor;
    driver_config.warn = driver::Warn::new({
        let shared = parts.shared.clone();
        move |message| shared.warn(message)
    });
    listener.stage_complete(Stage::ControlStreamInit);

    listener.stage_starting(Stage::VideoStreamInit);
    let stream = &outcome.config;
    let video_encrypted = outcome.encryption_enabled & sdp::SS_ENC_VIDEO != 0;
    let video_shared = Arc::new(video::VideoShared::new(VideoReceiver::new(VideoConfig {
        format: outcome.negotiated_video_format,
        packet_size: stream.packet_size as usize,
        app_version: outcome.app_version,
        rfi: outcome.reference_frame_invalidation_enabled(),
        pyrowave_records: outcome.pyrowave_records_enabled,
        colorspace: stream.color_space as u8,
        encryption_key: video_encrypted.then_some(stream.remote_input_aes_key),
        early_chunks: video_decoder.wants_early_chunks(),
    })));
    parts.video_shared = Some(video_shared.clone());
    listener.stage_complete(Stage::VideoStreamInit);

    listener.stage_starting(Stage::InputStreamInit);
    listener.stage_complete(Stage::InputStreamInit);

    listener.stage_starting(Stage::ControlStreamStart);
    let options = SessionOptions {
        reference_frame_invalidation: outcome.reference_frame_invalidation_enabled(),
    };
    let (client, control) =
        driver::connect(driver_config, caps, &stream.remote_input_aes_key, options).map_err(
            |e| {
                let code = match &e {
                    driver::ConnectError::Io(io) => io_code(io),
                    _ => error::FAILED,
                };
                fail(Stage::ControlStreamStart, code, e.to_string())
            },
        )?;
    parts.control = Some(control.clone());
    let events = parts.shared.clone();
    let hdr_video = video_shared.clone();
    parts.control_thread = Some(
        std::thread::Builder::new()
            .name("ControlRecv".into())
            .spawn(move || {
                events.prioritize_thread();
                client.run(|event| match event {
                    ControlEvent::Terminated { error } => events.terminate(error),
                    event => {
                        if let ControlEvent::HdrMode { enabled, .. } = event {
                            hdr_video
                                .receiver
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .set_hdr_active(enabled);
                        }
                        events.listener.control_event(event);
                    }
                })
            })
            .map_err(|e| fail(Stage::ControlStreamStart, io_code(&e), e.to_string()))?,
    );
    listener.stage_complete(Stage::ControlStreamStart);
    check_interrupt(Stage::ControlStreamStart)?;

    // Input-only sessions never set up or clean up the decoder and renderer. They still ping
    // the video port: Vibepollo and Apollo hosts run their video thread regardless and end
    // the session when the first ping does not arrive within ten seconds.
    listener.stage_starting(Stage::VideoStreamStart);
    if config.input_only {
        // Pings are best effort: a refused or failed send is ignored, nothing reads replies.
        match net::bind_udp(local, 0, Qos::Video, config.network_priority, |m| {
            listener.warn(m)
        })
        .and_then(|socket| {
            net::Pinger::start(
                "VideoPing",
                &socket,
                SocketAddr::new(remote.ip(), outcome.video_port),
                outcome.video_ping_payload,
            )
        }) {
            Ok(pinger) => parts.video_pinger = Some(pinger),
            Err(e) => listener.warn(&format!("Could not start the input-only video ping: {e}")),
        }
    } else {
        let mut decoder = video_decoder;
        let coded = if stream.experimental_packed_444 {
            rubylight_protocol::packed444::Geometry::new(stream.width as u32, stream.height as u32)
                .expect("validated packed geometry")
                .coded_size()
        } else {
            (stream.width as u32, stream.height as u32)
        };
        let setup = VideoSetup {
            format: outcome.negotiated_video_format,
            width: coded.0 as i32,
            height: coded.1 as i32,
            fps: stream.fps,
            chroma_packing: if stream.experimental_packed_444 {
                stream.packed444_version
            } else {
                0
            },
            logical_width: stream.width,
            logical_height: stream.height,
            color_space: stream.color_space,
            color_range: stream.color_range,
        };
        let video_fail = |code: i32, message: String| fail(Stage::VideoStreamStart, code, message);
        if let Err(code) = decoder.setup(&setup) {
            return Err(video_fail(code, "the video decoder setup failed".into()));
        }
        let buffer = crate::video::recv_buffer_size(
            stream.bitrate_kbps.max(0) as u32,
            stream.packet_size as usize,
        );
        let socket = match net::bind_udp(local, buffer, Qos::Video, config.network_priority, |m| {
            listener.warn(m)
        }) {
            Ok(socket) => socket,
            Err(e) => {
                decoder.cleanup();
                return Err(video_fail(io_code(&e), e.to_string()));
            }
        };
        decoder.start();
        let start = video::VideoStart {
            socket,
            target: SocketAddr::new(remote.ip(), outcome.video_port),
            ping_payload: outcome.video_ping_payload,
            direct_submit: outcome.video_capabilities & sdp::CAPABILITY_DIRECT_SUBMIT != 0,
        };
        parts.video = Some(
            video::VideoStream::start(
                start,
                decoder,
                video_shared,
                parts.shared.clone(),
                control.clone(),
            )
            .map_err(|e| video_fail(io_code(&e), e.to_string()))?,
        );
    }
    listener.stage_complete(Stage::VideoStreamStart);

    listener.stage_starting(Stage::AudioStreamStart);
    if !config.input_only {
        let mut renderer = audio_renderer;
        let duration = u32::from(outcome.audio_packet_duration_ms);
        let opus = match (
            outcome.high_quality_surround_enabled,
            outcome.opus.high_quality,
        ) {
            (true, Some(high)) => high,
            _ => outcome.opus.normal,
        };
        let setup = AudioSetup {
            configuration: stream.audio,
            opus,
            packet_duration_ms: duration,
            samples_per_frame: 48 * duration,
        };
        let audio_fail = |code: i32, message: String| fail(Stage::AudioStreamStart, code, message);
        if let Err(code) = renderer.init(&setup) {
            return Err(audio_fail(code, "the audio renderer init failed".into()));
        }
        renderer.start();
        let receiver = AudioReceiver::new(AudioConfig {
            packet_duration_ms: duration,
            app_version: outcome.app_version,
            encryption: outcome.audio_encryption_enabled.then_some(AudioEncryption {
                key: stream.remote_input_aes_key,
                iv: stream.remote_input_aes_iv,
            }),
        });
        let Some(socket) = parts.audio_socket.as_ref() else {
            parts.audio_renderer = Some(renderer);
            return Err(audio_fail(
                error::FAILED,
                "no audio port was negotiated".into(),
            ));
        };
        parts.audio = Some(
            audio::AudioStream::start(
                socket,
                receiver,
                renderer,
                stream.audio_capabilities & sdp::CAPABILITY_DIRECT_SUBMIT != 0,
                parts.audio_shared.clone(),
                parts.shared.clone(),
            )
            .map_err(|e| audio_fail(io_code(&e), e.to_string()))?,
        );
    }
    listener.stage_complete(Stage::AudioStreamStart);

    listener.stage_starting(Stage::InputStreamStart);
    // The microphone comes up with the last stage; failing costs only the microphone.
    if outcome.mic_port != 0 {
        if outcome.encryption_enabled & sdp::SS_ENC_MICROPHONE == 0 {
            listener.log("Microphone: encryption was not negotiated; not sending");
        } else if let Ok(socket) =
            net::bind_udp(local, 0, Qos::Audio, config.network_priority, |m| {
                listener.warn(m)
            })
        {
            *parts.microphone.lock().unwrap_or_else(|p| p.into_inner()) = Some(Microphone {
                socket,
                target: SocketAddr::new(remote.ip(), outcome.mic_port),
                key: stream.remote_input_aes_key,
                key_id: u32::from_be_bytes(
                    stream.remote_input_aes_iv[..4].try_into().expect("4 bytes"),
                ),
                sequence: 0,
                started: Instant::now(),
            });
        }
    }
    listener.stage_complete(Stage::InputStreamStart);
    parts.outcome = Some(outcome);

    // Wiggle the mouse to wake the host's display; a remote monitor sends no input at all.
    if !config.remote_monitor {
        let _ = control.with_input(|q| q.send_mouse_move(1, 1));
        std::thread::sleep(Duration::from_millis(10));
        let _ = control.with_input(|q| q.send_mouse_move(-1, -1));
        std::thread::sleep(Duration::from_millis(10));
    }

    listener.connection_started();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packed_refusal_has_its_own_start_code_and_keeps_its_text() {
        let refused = RtspError::PackedRefused(
            "requested packed codec and bit depth are not supported by both endpoints".into(),
        );
        assert_eq!(rtsp_error_code(&refused), error::PACKED_444_REFUSED);
        assert_eq!(
            refused.to_string(),
            "RTSP: requested packed codec and bit depth are not supported by both endpoints"
        );
        // Every other failure keeps its code, so only a packed refusal reconnects without it.
        assert_eq!(
            rtsp_error_code(&RtspError::Unsupported("x".into())),
            error::FAILED
        );
        assert_eq!(
            rtsp_error_code(&RtspError::Status {
                request: "ANNOUNCE",
                code: 400,
                reason: "Bad Request".into(),
            }),
            400
        );
    }

    #[test]
    fn auto_location_caps_remote_packet_sizes() {
        let mut stream = StreamConfig {
            packet_size: 1392,
            ..Default::default()
        };
        decide_location(Location::Auto, "192.168.1.2".parse().unwrap(), &mut stream);
        assert_eq!(
            (stream.location, stream.packet_size),
            (StreamLocation::Local, 1392)
        );
        decide_location(Location::Auto, "8.8.8.8".parse().unwrap(), &mut stream);
        assert_eq!(
            (stream.location, stream.packet_size),
            (StreamLocation::Remote, 1024)
        );
        stream.packet_size = 1392;
        decide_location(Location::Local, "8.8.8.8".parse().unwrap(), &mut stream);
        assert_eq!(
            (stream.location, stream.packet_size),
            (StreamLocation::Local, 1392)
        );
    }

    #[test]
    fn stage_names_match_the_c() {
        assert_eq!(Stage::RtspHandshake.name(), "RTSP handshake");
        assert_eq!(Stage::InputStreamStart as i32, 11);
    }
}
