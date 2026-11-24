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
mod gauges;
mod net;
mod roam;
mod shared;
mod start;
mod trace;
mod usb;
mod video;

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub use audio::AUDIO_QUEUE_BOUND;
pub use gauges::{AudioGauges, VideoGauges};
pub use net::{is_nat64_with, is_private, ReceiveBuffer};
pub use roam::{NetworkChange, NetworkHint, RoamingConfig};
pub(crate) use shared::Shared;
pub use video::DECODE_UNIT_QUEUE_BOUND;

use shared::{io_code, HostWatch, Microphone, Parts};
use start::start;

use crate::audio::AudioStats;
use crate::control::driver::ControlHandle;
use crate::control::{
    ClientMessage, ConnectionStatus, ControlEvent, HdrMetadata, SendError,
};
use crate::input::microphone;
use crate::input::InputQueue;
use crate::rtsp::{OpusConfig, RtspOutcome};
use crate::sdp::{AudioConfiguration, StreamConfig};
use crate::video::{Frame, VideoStats};

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
    pub const PACKED_444_REFUSED: i32 = nightjar_policy::full_chroma::PACKED_444_REFUSED;
    /// The PC is gone: its host said goodbye and closed its server (the PC shuts down, the host
    /// stopped or restarts; [`crate::host_loss`], Java's `CLIENT_ERROR_HOST_GONE`). Nothing
    /// reconnects on its own.
    pub const HOST_GONE: i32 = -108;
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
    /// [`crate::control::driver::DriverConfig::network_grace`].
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
    /// The decode priority switch's setting ([`crate::priority::decode_priority_switch`]): with
    /// direct submission and `network_priority`, VideoRecv runs at the decoder's level, and on
    /// Windows the session threads are raised. On for the desktop; Android's Debug setting
    /// "Decoder priority for receiving", on by default there and passed in by the app.
    pub receive_decode_priority: bool,
    /// Nightjar's "Group incoming video packets": the network stack may hand over several
    /// video datagrams in one read (UDP_GRO on Linux and Android, receive coalescing on
    /// Windows). On by default in the apps; off, each datagram is read on its own (Linux and
    /// Android still take `recvmmsg` batches). A Windows adapter that loses the size of a
    /// coalesced read turns it off for the stream by itself (`net::BatchReceiver`).
    pub receive_coalescing: bool,
    /// Nightjar's "Frames waiting for the decoder": complete frames that may wait for a decoder
    /// without direct submission before the queue overflows, is flushed and a key frame is
    /// requested. 0 (the default) keeps moonlight-common-c's 15, and so does anything larger
    /// ([`video::decode_queue_bound`]). A direct-submit decoder has no queue.
    pub decode_queue_bound: usize,
    /// Client roaming (Experimental), with hosts that advertise it.
    pub roaming: RoamingConfig,
    /// Frame Warp: the session counts input for it, asks a host that lists 0x5543 for its
    /// model, tags and HUD mask, and feeds them in. The app hands the same state to its
    /// presenter.
    pub frame_warp: Option<Arc<crate::frame_warp::FrameWarp>>,
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
    /// Every frame decodes without the one before it (PyroWave), so a frame the decoder is
    /// behind on may give way to a newer one already queued: it then goes to
    /// [`VideoDecoder::skip`] instead of [`VideoDecoder::submit`], and early chunks of a frame
    /// that is certain to be skipped are dropped. Asked on the decoder thread, and only for a
    /// frame with a newer one queued behind it; never true for a decoder that predicts from
    /// earlier frames.
    fn intra_only(&self) -> bool {
        false
    }
    /// A frame of an [`VideoDecoder::intra_only`] decoder that a newer queued frame replaces.
    /// It must leave the decoder where decoding the frame would have (the state a later frame
    /// recovers its lost parts from) without producing a picture. Any early chunks taken for
    /// it are void.
    fn skip(&mut self, _frame: &Frame) {}
    /// Whether the decoder now decoding recovers with reference frame invalidation. Asked
    /// after each frame: a decoder replaced mid-stream by one that never asked for it answers
    /// false, and the rest of the stream recovers from losses with key frames only
    /// ([`crate::video::VideoReceiver::recover_with_idr_only`]).
    fn recovers_with_rfi(&self) -> bool {
        true
    }
    fn stop(&mut self) {}
    fn cleanup(&mut self) {}
}

/// The negotiated audio stream, for [`AudioRenderer::init`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioSetup {
    pub configuration: AudioConfiguration,
    /// The high-quality configuration when high-quality surround is on, else the normal one.
    pub opus: OpusConfig,
    /// The codec of the packets [`AudioRenderer::play`] gets, one `audio_ext::CODEC_*` bit:
    /// `CODEC_OPUS`, `CODEC_PCM` (s16le samples), or `CODEC_AC3` or `CODEC_EAC3` (one whole
    /// frame per packet). Only the platform's `StreamConfig::audio_passthrough` brings anything
    /// but Opus.
    pub codec: u8,
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
    /// A packet in [`AudioSetup::codec`] in order, or `None` for a lost one (run packet loss
    /// concealment, or play a packet's worth of silence).
    fn play(&mut self, packet: Option<&[u8]>);
    fn stop(&mut self) {}
    fn cleanup(&mut self) {}
}

/// Audio received and never played (Mute audio): the PC streams as it would, and no output is
/// opened. Takes every codec, passthrough ones too, so the stream starts whatever it negotiated.
pub struct SilentAudio;

impl AudioRenderer for SilentAudio {
    fn init(&mut self, _setup: &AudioSetup) -> Result<(), i32> {
        Ok(())
    }

    fn play(&mut self, _packet: Option<&[u8]>) {}
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
    /// The packed 4:4:4 modes the host's DESCRIBE offered (`packed444::mode_bit`, 0 without),
    /// called with [`Listener::host_control_messages`]: before the decoder's setup, so it can
    /// name why a 10-bit packed request streams 8-bit, and so the client can remember them for
    /// its next launch (which comes before DESCRIBE).
    fn host_packed444_modes(&self, _modes: u8) {}
    /// The HDR format this stream's ANNOUNCE asked for (one `rubylight_protocol::hdr` bit, 0 for
    /// stock HDR), called with [`Listener::host_packed444_modes`]: before the decoder's setup, so
    /// it decodes what the host was actually asked for.
    fn announced_hdr_format(&self, _format: u8) {}
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
    /// Frames an intra-only decoder that had fallen behind passed over for a newer queued one
    /// (not in `frames_submitted`), and the early chunks dropped with them.
    pub frames_skipped: u64,
    pub chunks_skipped: u64,
    pub decode_queue_len: usize,
    /// Opus packets and loss markers delivered, and the pending queue.
    pub audio_packets: u64,
    pub audio_queue_len: usize,
    pub audio_queue_overflows: u64,
    pub connection_status: Option<ConnectionStatus>,
    /// Input held back during a network grace period.
    pub input_contact_lost: bool,
}

/// A running stream. Dropping it stops it.
pub struct Session {
    parts: Parts,
}

/// [`Session::input`] from any thread ([`Session::input_handle`]); input sent after the
/// session stopped is refused, as on the session itself.
#[derive(Clone)]
pub struct InputHandle(ControlHandle);

impl InputHandle {
    pub fn input<R>(&self, f: impl FnOnce(&mut InputQueue) -> R) -> R {
        self.0.with_input(f)
    }
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
            shared: Arc::new(Shared::new(
                listener.clone(),
                config.network_priority,
                config.receive_decode_priority,
            )),
            remote: None,
            local: None,
            outcome: None,
            audio_socket: None,
            audio_pinger: None,
            video_pinger: None,
            video_shared: None,
            video_receive_buffer: None,
            audio_shared: Arc::new(audio::AudioShared::default()),
            control: None,
            control_thread: None,
            video: None,
            video_decoder: None,
            audio: None,
            audio_renderer: None,
            microphone: Arc::new(Mutex::new(None)),
            video_next_socket: Arc::default(),
            audio_next_socket: Arc::default(),
            roamer: None,
            roam_poll: None,
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
            Ok(()) => {
                parts.shared.started();
                Ok(Session { parts })
            }
            Err(error) => {
                // One report per attempt: the stage failure, never a termination beside it.
                if let Some(code) = parts.shared.start_failed() {
                    listener.log(&format!(
                        "The stream ended ({code}) while it started; reporting the start failure"
                    ));
                }
                listener.stage_failed(error.stage, error.code);
                parts.teardown();
                Err(error)
            }
        }
    }

    /// Whether the connection ended with this session's host's disconnect rather than a silence
    /// ([`ControlHandle::said_goodbye`]).
    pub fn host_said_goodbye(&self) -> bool {
        self.control().said_goodbye()
    }

    /// Readies the check after the host's goodbye ([`crate::host_loss`]): `port` is the PC's HTTPS
    /// port, `gateway` this device's gateway while the PC is on its link. A connection that ends
    /// with the goodbye of a host whose server then closes (the PC shuts down, the host stops or
    /// restarts) reports [`error::HOST_GONE`]. For clients that run the check here; the Android app
    /// runs its own over both of its engines.
    pub fn watch_host(&self, port: u16, gateway: impl Fn(IpAddr) -> Option<IpAddr> + Send + Sync + 'static) {
        let (Some(remote), Some(control)) = (self.parts.remote, self.parts.control.clone()) else {
            return;
        };
        *self.parts.shared.host_watch.lock().unwrap_or_else(|p| p.into_inner()) = Some(HostWatch {
            control,
            host: SocketAddr::new(remote.ip(), port),
            gateway: Arc::new(gateway),
        });
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

    /// The video socket's receive buffer as asked and as the OS granted it; None without video
    /// or when the size could not be read back.
    pub fn video_receive_buffer(&self) -> Option<ReceiveBuffer> {
        self.parts.video_receive_buffer
    }

    /// Bytes of video and audio datagrams received so far, FEC parity included: the stream's
    /// bandwidth on the wire, as Moonlight V+'s `LiGetRTPVideoBytesReceived` with audio added.
    pub fn wire_bytes(&self) -> u64 {
        self.parts.shared.wire_bytes.load(Ordering::Relaxed)
    }

    /// `LiGetPendingAudioDuration`: the milliseconds of audio queued for the renderer, counted
    /// in the stream codec's packets (32 ms AC3 frames, PCM's 1 to 5 ms packets, Opus's
    /// negotiated duration).
    pub fn pending_audio_ms(&self) -> u32 {
        self.parts.audio_shared.pending_ms()
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

    /// The input queue for another thread, such as the one reading the mouse.
    pub fn input_handle(&self) -> InputHandle {
        InputHandle(self.control().clone())
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
        let len = microphone::build_packet_into(
            &mic.cipher,
            mic.key_id,
            mic.sequence,
            timestamp,
            opus,
            &mut mic.packet,
        )
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad Opus packet size"))?;
        mic.sequence = mic.sequence.wrapping_add(1);
        mic.socket.send_to(&mic.packet[..len], mic.target).map(|_| ())
    }

    /// The front end's word that the network changed (Android's default network callback),
    /// and what it knows about the new one. Moves the sockets and starts a roam only when the
    /// local address toward the host changed; with the same address nothing moves, and a host
    /// that went silent starts its own attempt. On a metered network without
    /// [`RoamingConfig::metered`] roaming holds off and the network grace period decides.
    /// Returns whether a roam started, else whether the stream's connection carries on: without
    /// roaming on both sides it does while the route toward the host leaves from the address
    /// the session started on, or there is no route yet.
    pub fn network_changed(&self, hint: NetworkHint) -> NetworkChange {
        if let Some(roamer) = &self.parts.roamer {
            return roamer.network_changed(hint);
        }
        match self.parts.remote {
            Some(remote) => self.network_changed_on(net::local_address_toward(remote)),
            None => NetworkChange::CarriesOn,
        }
    }

    /// [`Session::network_changed`] without roaming, with the route toward the host given:
    /// for tests of a route that leaves from another address than the session started on.
    #[doc(hidden)]
    pub fn network_changed_on(&self, route: io::Result<SocketAddr>) -> NetworkChange {
        match self.parts.local {
            Some(local) => NetworkChange::standing(local, route),
            None => NetworkChange::CarriesOn,
        }
    }

    /// Simulates a roam (nightjar-cli's `--roam-at`): moves the control and media sockets to
    /// new ports on `local` (the current address without one), then starts a roam. Fails
    /// without roaming on both sides.
    pub fn roam_to(&self, local: Option<IpAddr>) -> io::Result<()> {
        self.parts
            .roamer
            .as_ref()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "roaming is off for this session (the client's setting or the host)",
                )
            })?
            .roam_to(local)
    }

    /// Moves only the control socket to `local` and starts a roam, leaving the media where
    /// they are: what [`Session::roam_to`] never does, for tests of the media following on
    /// Roamed. Fails without roaming on both sides.
    #[doc(hidden)]
    pub fn roam_control_only(&self, local: IpAddr) -> io::Result<()> {
        self.parts
            .roamer
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::Unsupported))?
            .roam_control_only(local)
    }

    /// Frames waiting in the decode queue now ([`SessionStats::decode_queue_len`] without the
    /// rest of the statistics): non-zero while the decoder is behind the stream.
    pub fn decode_queue_len(&self) -> usize {
        self.parts.video_shared.as_ref().map_or(0, |video| video.queue.len())
    }

    /// ENet's round-trip time and its variance, while connected.
    pub fn rtt(&self) -> Option<(Duration, Duration)> {
        self.control().round_trip_time()
    }

    /// The video figures a decoder may read while it decodes, None without video. They never
    /// take the receiver's lock, which a direct-submit decoder holds while it submits.
    pub fn video_gauges(&self) -> Option<VideoGauges> {
        self.parts.video_shared.clone().map(VideoGauges)
    }

    /// The audio figures a caller may read from any thread without taking any lock.
    pub fn audio_gauges(&self) -> AudioGauges {
        AudioGauges(self.parts.audio_shared.clone())
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
            stats.video = *video.stats.lock().unwrap_or_else(|p| p.into_inner());
            stats.frames_submitted = video.counters.submitted.load(Ordering::Relaxed);
            stats.frames_need_idr = video.counters.need_idr.load(Ordering::Relaxed);
            stats.decode_queue_overflows = video.counters.queue_overflows.load(Ordering::Relaxed);
            stats.early_chunks_dropped = video.counters.chunks_dropped.load(Ordering::Relaxed);
            stats.frames_skipped = video.counters.frames_skipped.load(Ordering::Relaxed);
            stats.chunks_skipped = video.counters.chunks_skipped.load(Ordering::Relaxed);
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

#[cfg(test)]
mod tests;
