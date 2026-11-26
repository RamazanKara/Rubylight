//! Stream media runtime: control/audio/video sockets, session startup, capture sharing,
//! and encoder configuration.
use crate::state::{Launch, Session, Shared};
mod app_window;
pub(crate) mod audio;
pub(crate) mod audio_encoder;
mod audio_haptics;
mod audio_sink;
mod background;
mod capture;
mod capture_worker;
use capture_worker::finish_worker;
pub mod clipboard_sync;
mod config;
mod control;
mod couch_audio;
mod device_recovery;
pub mod dvr;
#[cfg(test)]
mod encoder_tests;
pub(crate) mod frame_warp;
mod frame_warp_prior;
mod game_audio;
mod hdr;
mod instance_audio;
mod keyboard_layout;
mod qos;
mod roaming;
pub(crate) mod slices;
mod video;
use audio_sink::{AudioPace, AudioSink, GameStreamAudio, WebRtcAudio};
use butterpollo_media_policy::audio_passthrough::Codec;
#[cfg(test)]
use config::{apply_overrides, sets_peak};
pub use config::{
    cap_stream_resolution, device_config, effective_config, launch_app, launched_through_steam,
    report_stream_resolution_cap,
};
#[cfg(test)]
use control::{
    CONTROL_EVENT_LIMIT, ControlSocket, FEEDBACK_CHANNEL, INPUT_OVERFLOW, INPUT_QUEUE,
    MOTION_EVENT_REQUEST, PadSends, PeerConfig, RUMBLE_MARKER, RumbleForwarded,
    SWALLOWED_PER_RECEIVE, ScaleLookup, UNATTENDED_SWEEP, UnattendedSweep, control_wait_timeout,
    drop_uninjectable_input, end_unattended_sessions, expects_enet_control, expire_controllers,
    first_steam_attributes_refusal, haptics_negotiated, hold_pad_input, motion_requests,
    pad_reports, queue_input, server_command,
};
pub use control::{
    PadsWait, RUMBLE_DATA, RUMBLE_TRIGGER_DATA, attach_pads, controllers_back, controllers_wait,
    feedback_packets,
};
pub use instance_audio::{InstanceRoutes, Program, instance_routes, plays_on_pc, put_back};
#[cfg(test)]
use video::{
    COMPUTE_FAILURE_WINDOW, COMPUTE_RETRY_QUIET, ComputeDemotion, ENCODER_RECOVERY, EncoderOutput,
    EncoderRecovery, OUTPUT_POLL, QueryTrace, encoder_progress, next_output_poll,
    observe_foreground_game, poll_fits, wait_for_output,
};
#[cfg(any(test, feature = "test-support"))]
pub mod reconnect_harness;
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use butterpollo_crypto::crypto;
use butterpollo_input_policy::{
    controller_audio, haptics,
    input::{self, required_permission},
    text_input::Watch as TextWatch,
};
use butterpollo_media_policy::{
    encoder_backlog::{self, Backpressure},
    packet::{AudioPacketizer, VideoPacketizer},
};
use butterpollo_session::session::Role;
use butterpollo_win_graphics::device_loss::DeviceLost;
pub(crate) use hdr::require_hdr_source;
#[cfg(test)]
use hdr::should_fallback_hdr_to_sdr;

/// Backoff only after reopening fails; resource release is acknowledged.
const RECOVERY_RETRY: Duration = Duration::from_millis(150);
/// How long a capture of the app's window that became unavailable (minimised, closed) waits
/// for the app window worker, which checks every 250 ms, to lay the display back over it.
const WINDOW_TARGET_WAIT: Duration = Duration::from_millis(400);
/// What the stream card says after an Automatic streamed display moved to Desktop
/// Duplication because Windows Graphics Capture could not see an exclusive-fullscreen
/// app: the cause, what it costs and when it ends.
const DUPLICATION_WARNING: &str = "Windows Graphics Capture sent no picture while an exclusive-fullscreen app was in front, so this display is captured with Desktop Duplication for the rest of this session, which reads the display itself. Desktop Duplication costs what it always costs: HDR and capture timing can differ, and a turned monitor or a display another GPU drives cannot be captured that way. The next stream starts on the Capture method setting again.";
/// How long capture waits for the stream's display to come back before it
/// shows another one. Switching a virtual display back on takes about a
/// second; recreating it takes up to the driver's 10 s arrival deadline.
const DISPLAY_RETURN_WAIT: Duration = Duration::from_secs(15);
/// How often a still picture may ask Windows whether an exclusive-fullscreen
/// app is in front. Capture polls continuously, so the shell's answer runs
/// twice a second rather than on every idle poll.
const FULLSCREEN_PROBE: Duration = Duration::from_millis(500);
/// Which display a capture reopen uses.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ReopenOn {
    /// The stream's display.
    Stream,
    /// Not yet: the stream's display is not on the desktop.
    Wait,
    /// The primary display, after the stream's did not return in time.
    Primary,
}
/// Capture follows the stream's display. Windows can switch a virtual display
/// off (a game resetting the layout on exit); the heartbeat switches it back
/// on or recreates it and publishes it, possibly under a new name. Capture
/// waits for that and never quietly shows another desktop: only after
/// `DISPLAY_RETURN_WAIT` does a foreground stream open the primary display.
/// A background stream must never expose the local user's desktop.
fn reopen_on(
    present: bool,
    background: bool,
    missing_since: &mut Option<Instant>,
    now: Instant,
) -> ReopenOn {
    if present {
        *missing_since = None;
        return ReopenOn::Stream;
    }
    let since = *missing_since.get_or_insert(now);
    if !background && now.duration_since(since) >= DISPLAY_RETURN_WAIT {
        ReopenOn::Primary
    } else {
        ReopenOn::Wait
    }
}
/// The backend a capture reopens on: the stream's own (the configured one),
/// again after an ordinary reopen such as the secure desktop's, or Desktop
/// Duplication for the rest of the session once the fullscreen fallback moved
/// a frozen picture there. A backend that fell back for a while, as an
/// unavailable Rubylight display push does to Windows Graphics Capture, is
/// therefore tried again on the next reopen.
fn reopen_backend(kind: &str, switched_to_duplication: bool) -> &str {
    if switched_to_duplication { "ddx" } else { kind }
}
/// Whether a capture on Desktop Duplication reopens on Windows Graphics
/// Capture now that the desktop is visible again: it fell back because WGC
/// cannot capture a secure desktop (a UAC prompt, the lock screen), not
/// because the fullscreen fallback moved a frozen game picture there, which
/// keeps its source for the session, nor because the stream asked for Desktop
/// Duplication itself.
fn returns_to_wgc(
    available: bool,
    was_available: bool,
    capture_backend: &str,
    kind: &str,
    switched_to_duplication: bool,
) -> bool {
    available
        && !was_available
        && !switched_to_duplication
        && capture_backend == "ddx"
        && kind != "ddx"
}
/// Whether the fullscreen probe may ask the shell now: at most every
/// [`FULLSCREEN_PROBE`], however fast a still picture polls capture. The next
/// time is advanced before the answer is used, so a probe that finds nothing
/// waits as long as one that switches. `next` starts at the stream's first
/// frame, which lets the first probe through at once.
fn fullscreen_probe_due(now: Instant, next: &mut Instant) -> bool {
    if now < *next {
        return false;
    }
    *next = now + FULLSCREEN_PROBE;
    true
}
/// How long a stream waits for its capture to start: past
/// [`SAME_WINDOW_WAIT`] only for the try of a window the game opened after a
/// whole helper start went by on the one it replaced (#59).
const CAPTURE_START_WAIT: Duration = Duration::from_secs(12);
/// What a window the game still shows gets, tries included: one that sends no
/// frame in a whole helper start fails with its own error, before a client
/// gives up on seeing video and for the next player's capture to go ahead.
const SAME_WINDOW_WAIT: Duration = Duration::from_secs(10);
/// What a try of a game window's capture takes beyond its WGC helper's start.
const WINDOW_OPEN_MARGIN: Duration = Duration::from_secs(1);
/// Whether a game window's first capture, whose stream began waiting for it
/// at `started`, is tried again at `now`: not once the stream stops, and only
/// while the wait before the try and the try itself, with a WGC helper's
/// start of up to [`butterpollo_win_graphics::capture::WGC_START_TIMEOUT`], end
/// before the stream stops waiting, which is [`CAPTURE_START_WAIT`] when the
/// game has `replaced` the window just tried and [`SAME_WINDOW_WAIT`] when it
/// has not. A try that ran past it would end the stream with a timeout
/// instead of the capture's own error.
fn window_open_again(started: Instant, stop: &AtomicBool, now: Instant, replaced: bool) -> bool {
    let wait = if replaced {
        CAPTURE_START_WAIT
    } else {
        SAME_WINDOW_WAIT
    };
    !stop.load(Ordering::Acquire)
        && now
            + RECOVERY_RETRY
            + butterpollo_win_graphics::capture::WGC_START_TIMEOUT
            + WINDOW_OPEN_MARGIN
            <= started + wait
}
/// [`window_open_again`] now, after the wait between tries when it is.
fn wait_to_open_again(started: Instant, stop: &AtomicBool, replaced: bool) -> bool {
    if !window_open_again(started, stop, Instant::now(), replaced) {
        return false;
    }
    thread::sleep(RECOVERY_RETRY);
    true
}
/// Opens a stream's first capture, on the target `target` names when each
/// try starts. A game window's is tried again while `again` (told the target
/// that failed, it waits between tries) allows: at its start a game replaces
/// its splash window, or resizes its window as the capture opens, and that
/// open fails while the next one works (#56: three of five Nucleus Co-op
/// streams ended at once). A display's first failure is the stream's.
fn open_first<T>(
    mut target: impl FnMut() -> (String, u64),
    mut open: impl FnMut(&(String, u64)) -> Result<T>,
    mut again: impl FnMut(&(String, u64)) -> bool,
) -> Result<(T, (String, u64))> {
    loop {
        let next = target();
        let error = match open(&next) {
            Ok(capture) => return Ok((capture, next)),
            Err(error) => error,
        };
        let window = butterpollo_win_graphics::capture::WindowTarget::parse(&next.0)
            .is_ok_and(|window| window.is_some());
        if !window || !again(&next) {
            return Err(error);
        }
        tracing::warn!(target = %next.0, error = %format!("{error:#}"), "the game window's capture did not open; trying again");
    }
}

use crate::win_bridge as wb;
use butterpollo_win_graphics::{
    capture::{Capture, ComGuard, GpuImage, Priority},
    encoder::Encoder,
};
use butterpollo_win_io::{
    audio::{Loopback, Opus},
    input::{Injector, PadReport},
    net::ReplySocket,
};
use rusty_enet::{Event, Host, HostSettings, Packet, PacketKind, PeerID, PeerState};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type Latest = capture::Latest<GpuImage>;
#[cfg(test)]
use capture_worker::capture_config;
use capture_worker::{CaptureKey, Source};
/// Each session's video and audio addresses from its pings, by launch ID. The stream and
/// audio threads look theirs up for every frame and packet, so a lookup copies no key.
#[derive(Default)]
pub struct Peers(HashMap<String, [Option<SocketAddr>; 2]>);
impl Peers {
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn get(&self, id: &str, audio: bool) -> Option<SocketAddr> {
        self.0.get(id).and_then(|pair| pair[usize::from(audio)])
    }
    /// The address it replaced, if any.
    fn insert(&mut self, id: &str, audio: bool, peer: SocketAddr) -> Option<SocketAddr> {
        if let Some(pair) = self.0.get_mut(id) {
            return pair[usize::from(audio)].replace(peer);
        }
        let mut pair = [None; 2];
        pair[usize::from(audio)] = Some(peer);
        self.0.insert(id.to_owned(), pair);
        None
    }
    /// Forgets a session's addresses, returning each as (audio, address).
    fn remove(&mut self, id: &str) -> impl Iterator<Item = (bool, SocketAddr)> + use<> {
        let pair = self.0.remove(id).unwrap_or_default();
        [false, true]
            .into_iter()
            .zip(pair)
            .filter_map(|(audio, peer)| Some((audio, peer?)))
    }
    /// Whether any session's video or audio comes from `peer`.
    fn contains(&self, peer: SocketAddr) -> bool {
        self.0.values().flatten().any(|known| *known == Some(peer))
    }
}
/// The active session a media ping from `peer` is for, and how often it had roamed
/// then: its own ping payload, or a bare "PING" when only one session plays to that
/// IP and, once it roamed, from the address that session already uses.
fn ping_session(
    sessions: &crate::state::Sessions,
    peers: &Mutex<Peers>,
    ping: &[u8],
    peer: SocketAddr,
    is_audio: bool,
) -> Option<(Arc<Session>, u32)> {
    let ip = peer.ip().to_canonical();
    let from_ip = |s: &&Arc<Session>| s.client_ip() == ip;
    let only_one = sessions.active.values().filter(from_ip).count() == 1;
    let s = sessions.active.values().filter(from_ip).find(|s| {
        if ping.len() >= 16 && crypto::equal(&ping[..16], s.launch.ping.as_bytes()) {
            return true;
        }
        // Once a client negotiated the session ping payload (ML_FF_SESSION_ID_V1 = 0x02),
        // never fall back to bare "PING", which can be guessed or spoofed behind NAT.
        // After a roam only the session's own ping proves a new address: behind a
        // CGNAT anyone may share the client's IP.
        ping == b"PING"
            && (s.config.moonlight_features & 0x02) == 0
            && only_one
            && (s.roams() == 0 || peers.lock().unwrap().get(&s.launch.id, is_audio) == Some(peer))
    })?;
    Some((s.clone(), s.roams()))
}
/// Whether `s`, found by [`ping_session`] when it had roamed `roams` times, is still
/// active and has not roamed since.
fn still_current(sessions: &crate::state::Sessions, s: &Arc<Session>, roams: u32) -> bool {
    sessions
        .active
        .get(&s.launch.id)
        .is_some_and(|active| Arc::ptr_eq(active, s))
        && s.roams() == roams
}
/// Takes a media ping from `peer`, which reached this host on `local`: its session
/// learns the address, and replies to it leave from `local` (`answer_from`). That can
/// look up a route, so it runs with the sessions lock released, which launches, the
/// control worker and roams take. A teardown or roam in between is decided again
/// under the lock, as if the ping came after it. Returns the session whose video
/// now takes a new path.
fn take_media_ping(
    h: &Shared,
    m: &Media,
    socket: &ReplySocket,
    is_audio: bool,
    ping: &[u8],
    peer: SocketAddr,
    local: Option<IpAddr>,
) -> Option<Arc<Session>> {
    let found = ping_session(&h.sessions.lock().unwrap(), &m.peers, ping, peer, is_audio);
    let (found, roams) = found?;
    // Answer from the address the client reached: Windows may route a reply out of a
    // VPN address.
    socket.answer_from(peer, local);
    let sessions = h.sessions.lock().unwrap();
    let s = if still_current(&sessions, &found, roams) {
        found
    } else if let Some((s, _)) = ping_session(&sessions, &m.peers, ping, peer, is_audio) {
        socket.answer_from(peer, local);
        s
    } else {
        // What the answer pinned no longer belongs to anyone.
        if !m.peers.lock().unwrap().contains(peer) {
            socket.forget(peer);
        }
        return None;
    };
    let previous = {
        let mut peers = m.peers.lock().unwrap();
        peers.insert(&s.launch.id, is_audio, peer)
    };
    if previous == Some(peer) {
        return None;
    }
    if let Some(previous) = previous {
        socket.forget(previous);
    }
    tracing::info!(
        client = %s.launch.client.name,
        %peer,
        local = %describe_local(local),
        stream = if is_audio { "audio" } else { "video" },
        "stream peer"
    );
    (!is_audio && s.take_new_path()).then_some(s)
}
pub struct Media {
    video: Arc<ReplySocket>,
    audio: Arc<ReplySocket>,
    control_wake: Arc<UdpSocket>,
    control_wake_receiver: UdpSocket,
    /// The microphone receiver's counts; None when its port could not be bound.
    pub mic: Option<Arc<crate::mic::Counters>>,
    pub peers: Mutex<Peers>,
    captures: Mutex<HashMap<CaptureKey, Weak<Source>>>,
    control_port: u16,
    bind: IpAddr,
    /// The control worker's answer to [`Media::clients_told_of_shutdown`].
    shutdown_requested: AtomicBool,
    shutdown_told: std::sync::atomic::AtomicUsize,
    shutdown_elapsed_ms: std::sync::atomic::AtomicU64,
    shutdown_done: AtomicBool,
    /// Roaming seqs of the launches, kept across control worker restarts so no launch key
    /// repeats a nonce.
    roam_seqs: butterpollo_session::roaming::SeqMemory,
    /// Control-stream AES-GCM send sequence & receive replay window per launch id and key,
    /// kept across control worker restarts so no launch repeats a nonce or resets replay state.
    pub(crate) control_seqs: ControlSeqMemory,
}

#[derive(Default)]
pub(crate) struct ControlSeqState {
    pub(crate) sequence: u32,
    pub(crate) received: butterpollo_media_policy::packet::ReplayWindow,
}

type ControlSeqMap = HashMap<(String, [u8; 16]), Arc<Mutex<ControlSeqState>>>;

/// Control encryption sequences and replay windows per `(launch.id, launch.key)`, kept across
/// control worker restarts so a live launch never repeats an AES-GCM nonce or clears its replay
/// window while distinct launches starting with an identical key begin cleanly.
#[derive(Clone, Default)]
pub(crate) struct ControlSeqMemory(Arc<Mutex<ControlSeqMap>>);

impl ControlSeqMemory {
    pub(crate) fn for_launch(&self, id: &str, key: [u8; 16]) -> Arc<Mutex<ControlSeqState>> {
        Arc::clone(
            self.0
                .lock()
                .unwrap()
                .entry((id.to_owned(), key))
                .or_insert_with(|| Arc::new(Mutex::new(ControlSeqState::default()))),
        )
    }

    pub(crate) fn retain_launches<'a>(
        &self,
        live: impl IntoIterator<Item = (&'a str, &'a [u8; 16])>,
    ) {
        let keys: Vec<(&str, [u8; 16])> = live.into_iter().map(|(id, key)| (id, *key)).collect();
        self.0
            .lock()
            .unwrap()
            .retain(|(id, key), _| keys.iter().any(|(k_id, k_key)| k_id == id && k_key == key));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.lock().unwrap().is_empty()
    }
}
/// The first video ping from the path a client roamed to: the client needs a keyframe,
/// and what the link-rate controller learned about the old path no longer holds.
fn restart_on_new_path(s: &Session, c: &Config) {
    s.new_network_path();
    s.request_host_keyframe();
    let mut stream = s.config.clone();
    s.requested_mode()
        .apply_to(&mut stream, c.max_stream_resolution());
    s.rebuild_link_rate(&stream, c.link_rate_adaptation(), max_bitrate_kbps(s, c));
    rebuild_congestion(s, &stream, c);
}
/// The stream's video FEC at its start: the configured percentage, for every
/// frame (`fec_percentage`, PyroWave included unless it keeps a share of its
/// own), unless the client chose its own with `rlFecPercentage`.
fn configure_video_fec(s: &Session, c: &Config) -> Result<()> {
    let requested_fec = c.integer("fec_percentage", 20);
    if requested_fec != requested_fec.clamp(0, 100) {
        s.launch.warnings.set("network_fec_config", format!("FEC percentage {requested_fec} is outside 0-100; using {}%. Correct fec_percentage in Video settings.", requested_fec.clamp(0, 100)));
    }
    let fec_ceiling = if s.config.codec == 3 {
        c.pyrowave_fec_percentage().clamp(0, 255) as usize
    } else {
        requested_fec.clamp(0, 100) as usize
    };
    // A client's `rlFecPercentage` was checked at /launch.
    let choice = butterpollo_media_policy::extensions::fec_choice(&s.launch.options)?;
    let maximum = butterpollo_config::config::FEC_MAX_PERCENTAGE;
    let start = s.configure_fec_choice(choice, false, fec_ceiling, maximum);
    if choice != butterpollo_media_policy::extensions::FecChoice::Host {
        tracing::info!(
            client = %s.launch.client.name,
            choice = %choice.as_param(),
            pyrowave = s.config.codec == 3,
            percent = start,
            maximum,
            "client fec choice"
        );
    }
    Ok(())
}
/// Packet-level congestion control from the client's arrival feedback: `(enabled, shadow)`.
/// `congestion_control` turns it on; `BUTTERPOLLO_CONGESTION_SHADOW=1` runs it measure-only
/// for the A/B (`rust/release/congestion_ab.py`): it logs what it would do, applies nothing.
pub(crate) fn congestion_mode(c: &Config) -> (bool, bool) {
    let shadow = std::env::var("BUTTERPOLLO_CONGESTION_SHADOW").is_ok_and(|v| v == "1");
    (c.congestion_control() || shadow, shadow)
}
/// Congestion control for a stream whose mode or path changed; call after
/// `rebuild_link_rate`.
fn rebuild_congestion(
    s: &Session,
    stream: &butterpollo_media_policy::rtsp::Negotiated,
    c: &Config,
) {
    let (enabled, shadow) = congestion_mode(c);
    s.rebuild_congestion(stream, enabled, max_bitrate_kbps(s, c), shadow);
}
/// The user's `max_bitrate`, within what the stream's codec can be asked for:
/// PyroWave goes to 2 Gbps, the others to 500 Mbps.
fn max_bitrate_kbps(s: &Session, c: &Config) -> u32 {
    let maximum = if s.config.codec == 3 {
        2_000_000
    } else {
        500_000
    };
    c.integer("max_bitrate", 0).clamp(0, maximum) as u32
}
/// The local address a client's datagrams arrived on, for logs.
fn describe_local(local: Option<IpAddr>) -> String {
    local.map_or_else(|| "unknown".into(), |local| local.to_string())
}
/// Whether a stream's video socket refuses to fragment its datagrams: never.
/// The video socket is process-wide, so a flag set for one jumbo stream would
/// govern every stream, including a stock client behind a WireGuard or
/// Tailscale tunnel whose 1280-byte path would refuse every 1400-byte
/// datagram. The jumbo offer checks the route's MTU, and the PyroWave UDP
/// probe sets the flag on its own socket to test the path with the stream's
/// packet size before a stream starts.
pub(crate) fn video_refuses_fragmentation() -> bool {
    false
}

impl Media {
    /// The address the host listens on: unspecified unless it was bound to one.
    pub fn bind(&self) -> IpAddr {
        self.bind
    }

    /// Host shutdown: the control worker tells every client that its stream ends, its
    /// forwarded USB devices detaching first, through the path a normal stop uses. Wakes
    /// the worker and waits up to `wait` for it. Returns the clients told and how long
    /// telling them took, in milliseconds; `None` when the worker had not finished then.
    pub fn clients_told_of_shutdown(&self, wait: Duration) -> Option<(usize, u64)> {
        self.shutdown_requested.store(true, Ordering::Release);
        let _ = self.control_wake.send(&[1]);
        let deadline = Instant::now() + wait;
        while !self.shutdown_done.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(2));
        }
        Some((
            self.shutdown_told.load(Ordering::Acquire),
            self.shutdown_elapsed_ms.load(Ordering::Acquire),
        ))
    }

    pub fn new(h: Shared, bind: IpAddr) -> Result<Arc<Self>> {
        let ports = h.config.read().unwrap().ports()?;
        let video = ReplySocket::new(crate::network::udp((bind, ports.video).into())?)?;
        let audio = ReplySocket::new(crate::network::udp((bind, ports.audio).into())?)?;
        butterpollo_win_io::net::configure_udp(&video)?;
        butterpollo_win_io::net::configure_udp(&audio)?;
        video.set_nonblocking(true)?;
        audio.set_nonblocking(true)?;
        let control_wake_receiver = UdpSocket::bind("127.0.0.1:0")?;
        let control_wake = UdpSocket::bind("127.0.0.1:0")?;
        control_wake.connect(control_wake_receiver.local_addr()?)?;
        control_wake_receiver.connect(control_wake.local_addr()?)?;
        control_wake.set_nonblocking(true)?;
        control_wake_receiver.set_nonblocking(true)?;
        // A microphone port taken by another program only costs the microphone.
        let mic = crate::network::udp((bind, ports.mic).into())
            .and_then(|socket| crate::mic::spawn(h.clone(), socket))
            .inspect_err(|error| {
                tracing::warn!(port = ports.mic, error = %format!("{error:#}"), "client microphones unavailable")
            })
            .ok();
        let m = Arc::new(Self {
            video: Arc::new(video),
            audio: Arc::new(audio),
            control_wake: Arc::new(control_wake),
            control_wake_receiver,
            mic,
            peers: Mutex::default(),
            captures: Mutex::new(HashMap::new()),
            control_port: ports.control,
            bind,
            shutdown_requested: AtomicBool::new(false),
            shutdown_told: std::sync::atomic::AtomicUsize::new(0),
            shutdown_elapsed_ms: std::sync::atomic::AtomicU64::new(0),
            shutdown_done: AtomicBool::new(false),
            roam_seqs: Default::default(),
            control_seqs: Default::default(),
        });
        for is_audio in [false, true] {
            let socket = if is_audio {
                m.audio.clone()
            } else {
                m.video.clone()
            };
            let m = m.clone();
            let h = h.clone();
            thread::Builder::new()
                .name(if is_audio { "audio-ping" } else { "video-ping" }.into())
                .spawn(move || {
                    let mut b = [0; 2048];
                    let raw_socket = std::os::windows::io::AsRawSocket::as_raw_socket(&*socket);
                    while !h.stop.load(Ordering::Acquire) {
                        match butterpollo_win_io::net::wait_readable(raw_socket, 250) {
                            Ok(true) => {}
                            Ok(false) => continue,
                            Err(e) => {
                                tracing::warn!(error=%e,"media ping wait failed");
                                thread::sleep(Duration::from_millis(250));
                                continue;
                            }
                        }
                        match socket.receive(&mut b) {
                            Ok((n, peer, local)) => {
                                let roamed = take_media_ping(
                                    &h,
                                    &m,
                                    &socket,
                                    is_audio,
                                    &b[..n],
                                    peer,
                                    local,
                                );
                                if let Some(s) = roamed {
                                    restart_on_new_path(&s, &h.config.read().unwrap());
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(e) => {
                                tracing::warn!(error=%e,"media ping receive failed");
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                    }
                })?;
        }
        let control = m.clone();
        thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                // Input and control for every session pass through this
                // worker: restart it rather than leave later sessions without.
                while !h.stop.load(Ordering::Acquire) {
                    match control.control(h.clone()) {
                        Ok(()) => break,
                        Err(e) => {
                            tracing::error!(error = %format!("{e:#}"), "control worker stopped; restarting");
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                }
            })?;
        Ok(m)
    }
    pub fn start(self: &Arc<Self>, h: Shared, s: Arc<Session>) {
        // Raised first, so a session that ends at once can't end before it started.
        h.session_webhook("session.started", &s);
        let result = self.spawn_session(&h, &s);
        if let Err(e) = result {
            let reason = format!("the stream could not start: {e:#}");
            h.sessions.lock().unwrap().fail_start(&s, reason);
            h.record_failure(&s);
            h.record_session(
                &s,
                butterpollo_session::session_history::REASON_START_FAILED,
            );
            tracing::error!(error=%e,"could not start session worker");
            h.session_webhook("session.ended", &s);
        }
    }
    fn open_session_capture<'a>(
        &self,
        s: &Arc<Session>,
        c: &Config,
        prepared: &Arc<crate::display_session::Ready>,
        latest: &'a mut Option<Arc<Source>>,
    ) -> Result<(&'a mut Arc<Source>, Arc<capture::Consumer>)> {
        let latest = latest.insert(self.capture(
            s.config.hdr,
            c,
            butterpollo_media_policy::framegen::Rate(s.config.fps_millihz()),
            &s.launch.id,
            prepared.clone(),
        )?);
        *s.capture_warnings.write().unwrap() = latest.warnings.clone();
        let capture_wake = latest.subscribe(Arc::downgrade(s))?;
        capture_wake.wake_on_recovery(s);
        Ok((latest, capture_wake))
    }
    fn configure_encoder(
        &self,
        s: &Session,
        c: &Config,
        first: &GpuImage,
        prepared: &crate::display_session::Ready,
        encoder: &mut Option<Encoder>,
    ) -> Result<()> {
        let stream = s.effective_config();
        *encoder = Some(wb::new_encoder_gpu(&stream, first, c, &s.launch.warnings)?);
        *s.encoder.write().unwrap() = encoder.as_ref().unwrap().backend().into();
        let source_refresh_hz =
            butterpollo_win_display::display::mode(&first.gpu.display.display_name)
                .ok()
                .map(|mode| mode.dmDisplayFrequency);
        let (requested_capture, constant_rate) = prepared.capture_method();
        tracing::info!(width=stream.width,height=stream.height,display_width=stream.display_size().0,display_height=stream.display_size().1,capped=stream.capped(),fps=f64::from(stream.fps_millihz())/1000.,requested_fps=stream.fps,bitrate_kbps=s.bitrate.load(Ordering::Relaxed),configured_bitrate_kbps=stream.configured_bitrate_kbps,codec=stream.codec,hdr=stream.hdr,ten_bit=stream.ten_bit(),full_range=stream.full_range(),color_matrix=stream.color_matrix(),vrr=stream.vrr_low_latency,packet_size=stream.packet_size,pyrowave_records=stream.pyrowave_records,pyrowave_row_order=stream.pyrowave_row_order,requested_capture=%requested_capture,constant_rate=constant_rate,requested_encoder=c.get("encoder","auto"),encoder=encoder.as_ref().unwrap().backend(),adapter=%first.gpu.display.adapter,source_width=first.width,source_height=first.height,source_refresh_hz,source_pixel=?first.pixel,"stream configured");
        if stream.chroma_packing != 0 {
            let (coded_width, coded_height) = stream.coded_size();
            tracing::info!(
                packing_version = stream.chroma_packing,
                codec = stream.codec,
                coded_width,
                coded_height,
                bit_depth = if stream.ten_bit() { 10 } else { 8 },
                "packed 4:4:4 stream"
            );
        }
        // The same floor link-rate adaptation stops at, measured on the project's test scenes.
        // Whole frames per second, as the client's minimum is, so 59.94 compares as 60.
        let minimum = butterpollo_media_policy::link_rate::pyrowave_floor_kbps(&stream);
        let recommended = butterpollo_media_policy::link_rate::pyrowave_recommended_kbps(&stream);
        let bitrate = s.bitrate.load(Ordering::Relaxed);
        if stream.codec == 3 && bitrate < minimum {
            tracing::warn!(
                bitrate_kbps = bitrate,
                minimum_kbps = minimum,
                recommended_kbps = recommended,
                "PyroWave bitrate is below the level where the project's test scenes kept detail; text and textures may lose detail. Raise the bitrate in Moonlight with network headroom, lower the resolution or frame rate, or use HEVC or AV1"
            );
        } else if stream.codec == 3 && bitrate < recommended {
            tracing::warn!(
                bitrate_kbps = bitrate,
                minimum_kbps = minimum,
                recommended_kbps = recommended,
                "PyroWave bitrate is below recommended: text and textures may lose detail. Quality depends on the picture; raise the bitrate in Moonlight with network headroom, or use HEVC or AV1"
            );
        }
        // HDR metadata describes the captured source, independent of receiver
        // brightness. RTX HDR's picture is generated at its own peak, not the
        // SDR monitor's; its enhancer reports the same from its first frame.
        let mut display_metadata = first.gpu.hdr_metadata();
        if let Some(peak) = crate::nvidia_enhance::generated_peak(c, stream.hdr, first.pixel) {
            display_metadata = display_metadata.generated(peak);
        }
        let metadata = self.set_display_hdr_metadata(s, display_metadata);
        if let Some(encoder) = encoder.as_mut() {
            encoder.set_hdr_metadata(metadata);
            encoder.set_hdr_target(s.hdr_target_nits());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
