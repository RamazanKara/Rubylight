//! Shared session state, termination gating, thread priority reporting, microphone state and
//! teardown ownership (`Parts`).

use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::{audio, error, net, roam, video, AudioRenderer, Listener, VideoDecoder};
use crate::control::driver::{self, ControlHandle};
use crate::input::microphone;
use crate::priority;
use crate::rtsp::{RtspError, RtspOutcome};

/// State every session thread shares.
pub(crate) struct Shared {
    pub(super) listener: Arc<dyn Listener>,
    pub(super) stopping: Arc<AtomicBool>,
    terminated: AtomicBool,
    /// A termination that came while [`super::Session::start`] still ran, held until it returns: a
    /// failed start reports only its stage failure, a started one reports the termination
    /// after [`Listener::connection_started`].
    start_gate: Mutex<StartGate>,
    /// "Prioritize stream traffic" ([`super::SessionConfig::network_priority`]).
    pub(super) network_priority: bool,
    /// The decode priority switch ([`priority::decode_priority_switch`]), read once per session.
    pub(super) decode_priority: bool,
    /// Bytes of every video and audio datagram received, FEC parity included: what the stream
    /// takes on the wire ([`super::Session::wire_bytes`]).
    pub(super) wire_bytes: AtomicU64,
    /// Set by [`super::Session::watch_host`]: what a connection that ended with the host's goodbye is
    /// checked with.
    pub(super) host_watch: Mutex<Option<HostWatch>>,
    /// What raising the stream's threads came to so far, each said once (`raise_thread`).
    pub(super) raise_outcomes: Mutex<Vec<String>>,
}

/// Where [`super::Session::watch_host`] looks for the PC.
#[derive(Clone)]
pub(super) struct HostWatch {
    pub(super) control: ControlHandle,
    pub(super) host: SocketAddr,
    pub(super) gateway: Arc<dyn Fn(IpAddr) -> Option<IpAddr> + Send + Sync>,
}

enum StartGate {
    Starting(Option<i32>),
    Started,
}

impl Shared {
    pub(super) fn new(
        listener: Arc<dyn Listener>,
        network_priority: bool,
        decode_priority: bool,
    ) -> Self {
        Self {
            listener,
            stopping: Arc::new(AtomicBool::new(false)),
            terminated: AtomicBool::new(false),
            start_gate: Mutex::new(StartGate::Starting(None)),
            network_priority,
            decode_priority: priority::decode_priority_switch(decode_priority),
            wire_bytes: AtomicU64::new(0),
            host_watch: Mutex::new(None),
            raise_outcomes: Mutex::new(Vec::new()),
        }
    }

    /// Counts a received video or audio datagram toward [`super::Session::wire_bytes`].
    pub(crate) fn received(&self, bytes: usize) {
        self.wire_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Reports a termination once, on its own thread, unless the session is stopping. While
    /// the session starts it is held instead (see [`Shared::start_gate`]).
    pub(crate) fn terminate(&self, code: i32) {
        if self.stopping.load(Ordering::SeqCst) || self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        if let StartGate::Starting(held) =
            &mut *self.start_gate.lock().unwrap_or_else(|p| p.into_inner())
        {
            *held = Some(code);
            return;
        }
        self.report_terminated(code);
    }

    fn report_terminated(&self, code: i32) {
        let listener = self.listener.clone();
        let watch = (code == error::DISCONNECTED)
            .then(|| self.host_watch.lock().unwrap_or_else(|p| p.into_inner()).clone())
            .flatten()
            .filter(|w| w.control.said_goodbye());
        let stopping = self.stopping.clone();
        let _ = std::thread::Builder::new()
            .name("AsyncTerm".into())
            .spawn(move || {
                // After this session's host said goodbye: a server that closed (a PC shutting
                // down, a host that stopped or restarts) ends here. A host that went silent past
                // the timeout is left to the recovery, like one this can't tell.
                let verdict = watch.map(|w| {
                    crate::host_loss::watch_goodbye(
                        w.host,
                        &*w.gateway,
                        &|| stopping.load(Ordering::SeqCst),
                        &|line| listener.log(line),
                    )
                });
                let gone = verdict == Some(crate::host_loss::Goodbye::Gone);
                listener.connection_terminated(if gone { error::HOST_GONE } else { code });
            });
    }

    /// The start succeeded: a termination held while it ran is reported now.
    pub(super) fn started(&self) {
        let gate = std::mem::replace(
            &mut *self.start_gate.lock().unwrap_or_else(|p| p.into_inner()),
            StartGate::Started,
        );
        if let StartGate::Starting(Some(code)) = gate
            && !self.stopping.load(Ordering::SeqCst)
        {
            self.report_terminated(code);
        }
    }

    /// The start failed: nothing more is reported, and the termination held while it ran (if
    /// any) is returned, so the caller can log it beside the stage failure.
    pub(super) fn start_failed(&self) -> Option<i32> {
        self.stopping.store(true, Ordering::SeqCst);
        match std::mem::replace(
            &mut *self.start_gate.lock().unwrap_or_else(|p| p.into_inner()),
            StartGate::Started,
        ) {
            StartGate::Starting(held) => held,
            StartGate::Started => None,
        }
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
        self.raise_thread(priority::StreamThread::Other);
    }

    /// Raises the video receive thread: to the decoder's level when it runs the decoder.
    pub(crate) fn prioritize_receive_thread(&self, direct_submit: bool) {
        self.raise_thread(priority::StreamThread::VideoReceive { direct_submit });
    }

    /// Raises the queued video decode thread; a refusal is logged, not fatal.
    pub(crate) fn prioritize_decode_thread(&self) {
        self.raise_thread(priority::StreamThread::VideoDecode);
    }

    /// Raises the calling thread, saying in the log what raised it or why nothing did once per
    /// outcome and stream: the stream's threads raised alike make one line, not one each.
    fn raise_thread(&self, thread: priority::StreamThread) {
        let level = priority::thread_level(thread, self.network_priority, self.decode_priority);
        let name = std::thread::current().name().unwrap_or("a stream thread").to_owned();
        let (outcome, line, warning) = match priority::raise_current_thread(level) {
            Ok(Some(raised)) => (format!("{raised}"), format!("Raised stream threads to {raised}, from {name} on"), false),
            Ok(None) => return,
            Err(e) => (format!("refused: {e}"), format!("Could not raise the priority of stream threads, from {name} on: {e}"), true),
        };
        if !first_outcome(&self.raise_outcomes, outcome) {
            return;
        }
        if warning { self.warn(&line) } else { self.log(&line) }
    }
}

/// Whether `outcome` is new to `seen`, which keeps it from then on.
pub(super) fn first_outcome(seen: &Mutex<Vec<String>>, outcome: String) -> bool {
    let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
    if seen.contains(&outcome) {
        return false;
    }
    seen.push(outcome);
    true
}

pub(super) struct Microphone {
    pub(super) socket: UdpSocket,
    pub(super) target: SocketAddr,
    pub(super) cipher: aes::Aes128,
    pub(super) key_id: u32,
    pub(super) sequence: u16,
    pub(super) started: Instant,
    pub(super) packet: [u8; microphone::MAX_PACKET_BYTES],
}

/// Everything a session owns; filled stage by stage so a failed start tears down exactly
/// what it built.
pub(super) struct Parts {
    pub(super) shared: Arc<Shared>,
    pub(super) remote: Option<SocketAddr>,
    /// The local address toward the host when the session started.
    pub(super) local: Option<SocketAddr>,
    pub(super) outcome: Option<RtspOutcome>,
    pub(super) audio_socket: Option<UdpSocket>,
    pub(super) audio_pinger: Option<net::Pinger>,
    /// An input-only session's video pings, with no receive side behind them.
    pub(super) video_pinger: Option<net::Pinger>,
    pub(super) video_shared: Option<Arc<video::VideoShared>>,
    /// The video socket's receive buffer, as asked and as granted.
    pub(super) video_receive_buffer: Option<net::ReceiveBuffer>,
    pub(super) audio_shared: Arc<audio::AudioShared>,
    pub(super) control: Option<ControlHandle>,
    pub(super) control_thread: Option<JoinHandle<driver::Outcome>>,
    pub(super) video: Option<video::VideoStream>,
    pub(super) video_decoder: Option<Box<dyn VideoDecoder>>,
    pub(super) audio: Option<audio::AudioStream>,
    pub(super) audio_renderer: Option<Box<dyn AudioRenderer>>,
    pub(super) microphone: Arc<Mutex<Option<Microphone>>>,
    /// Where a roam's rebind leaves the receive threads' next sockets.
    pub(super) video_next_socket: Arc<net::NextSocket>,
    pub(super) audio_next_socket: Arc<net::NextSocket>,
    /// Present when both sides have roaming on.
    pub(super) roamer: Option<Arc<roam::Roamer>>,
    pub(super) roam_poll: Option<JoinHandle<()>>,
}

impl Parts {
    /// LiStopConnection's order: input, audio, video, control, then the audio socket. The
    /// route poll stops first, so no roam moves a stream that is stopping.
    pub(super) fn teardown(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        if let Some(roamer) = &self.roamer {
            roamer.stop();
        }
        if let Some(thread) = self.roam_poll.take() {
            let _ = thread.join();
        }
        *self.microphone.lock().unwrap_or_else(|p| p.into_inner()) = None;
        if let Some(stream) = self.audio.take()
            && let Some(renderer) = stream.stop()
        {
            self.audio_renderer = Some(renderer);
        }
        if let Some(mut renderer) = self.audio_renderer.take() {
            renderer.stop();
            renderer.cleanup();
        }
        if let Some(stream) = self.video.take()
            && let Some(decoder) = stream.stop()
        {
            self.video_decoder = Some(decoder);
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

pub(super) fn rtsp_error_code(error: &RtspError) -> i32 {
    match error {
        RtspError::Status { code, .. } => *code,
        RtspError::Io(e) => e.raw_os_error().unwrap_or(error::FAILED),
        RtspError::PackedRefused(_) => error::PACKED_444_REFUSED,
        _ => error::FAILED,
    }
}

pub(super) fn io_code(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(error::FAILED)
}
