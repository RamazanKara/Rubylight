//! WebRTC sessions on the stream pipeline: `start-session`, `stop-session`, `switch-session` and
//! `stop-host-session` (`docs/webrtc-design.md`).
//!
//! One actor per signalling connection runs the requests in order. It validates a request
//! against the host's capabilities, launches the app and display through the same back half as a
//! Moonlight launch (`nvhttp::launch_webrtc`), offers the str0m session over the socket, and on
//! the client's answer starts the media transport and the stream worker. The worker takes the
//! transport from [`Links`] and sends each encoded access unit through a [`Sender`]; the
//! GameStream video, audio and ENet paths do not run for the session.
//!
//! A session ends when the client sends `stop-session`, the signalling socket closes, the media
//! transport closes, or the stream fails; the actor says `idle` (or `error`) when it notices.

use super::{
    input::{self, InputHandle},
    media::{
        self, AudioSender, Hooks, IdrReason, MediaHandle, Pacing, Submit, TransportEvent,
        TransportOptions, VideoFrame,
    },
    sdp::{self, AudioMode, OfferSpec, VideoCodec, VideoMode},
    server,
};
use crate::{
    nvhttp::{self, StopStreams, WebRtcLaunch, WebRtcLaunchError},
    state::{Launch, Session, Shared},
    stream::Media,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use butterpollo_core::{
    audio::OpusLayout,
    performance::{Claims, Timing},
    rtsp::{Negotiated, webrtc_audio_kbps},
    webrtc::{
        capabilities::{Codec, HostCapabilities, StreamSettings},
        protocol::{
            AudioSettings, ControlMessage, ErrorCode, HostSessionState, ServerMessage,
            SessionState, VideoSettings, classify_control,
        },
    },
};
use butterpollo_windows::encoder::{Encoded, Unit};
use std::{
    cell::Cell,
    collections::HashMap,
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// The permission to start and end streams, as for a Moonlight launch.
const LAUNCH: u32 = 1 << 26;
/// How long the client has to answer the offer. The launch it holds expires after 30 s.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);
/// How long an answered session may take to connect before it is ended.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How much longer than [`CONNECT_TIMEOUT`] a connected session may wait for the worker to vouch
/// for an HDR source before it is ended: a worker stuck before its first frame must not leave the
/// client in "starting" for good.
const HELD_EXTRA: Duration = Duration::from_secs(10);
/// How long a new session waits for the media port of the one before it.
const BIND_WAIT: Duration = Duration::from_secs(3);
/// How often the actor looks for a session that ended.
const WATCH: Duration = Duration::from_millis(100);
/// Capture instants of consecutive frames differ by at least one 90 kHz tick (11.1 us).
const MIN_STEP: Duration = Duration::from_micros(12);

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

/// The media transport of each session between its signalling task and its stream worker, and
/// the session ids the host hands out.
#[derive(Default)]
pub struct Links {
    media: Mutex<HashMap<String, MediaHandle>>,
    ids: AtomicU64,
    /// Test override of the held-"streaming" limit in milliseconds; 0 keeps the default.
    held_limit_ms: AtomicU64,
}

impl Links {
    /// How long "streaming" may stay held for an unchecked HDR source after the session started.
    fn held_limit(&self) -> Duration {
        match self.held_limit_ms.load(Ordering::Relaxed) {
            0 => CONNECT_TIMEOUT + HELD_EXTRA,
            ms => Duration::from_millis(ms),
        }
    }

    #[cfg(test)]
    pub fn limit_held(&self, limit: Duration) {
        self.held_limit_ms
            .store(micros(limit) / 1000, Ordering::Relaxed);
    }

    /// A positive id, unique for the life of the host.
    pub fn next_session_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn put(&self, launch: String, media: MediaHandle) {
        let replaced = self.media.lock().unwrap().insert(launch, media);
        // Joins the old transport's threads: never under the lock.
        drop(replaced);
    }

    pub fn take(&self, launch: &str) -> Option<MediaHandle> {
        self.media.lock().unwrap().remove(launch)
    }

    /// The audio end of a transport that has not been taken yet, for the audio thread. The video
    /// worker takes the handle itself later.
    pub fn audio_sender(&self, launch: &str) -> Option<AudioSender> {
        self.media.lock().unwrap().get(launch)?.audio_sender()
    }

    /// Drop a transport nobody took, freeing its port.
    pub fn discard(&self, launch: &str) {
        drop(self.take(launch));
    }

    pub fn len(&self) -> usize {
        self.media.lock().unwrap().len()
    }
}

/// A capture instant no earlier than one RTP tick after the last one: the transport maps the
/// instant to the 90 kHz timestamp and does not repair a frame that steps back or repeats.
fn monotonic(last: &Cell<Option<Instant>>, capture: Instant) -> Instant {
    let capture = match last.get() {
        Some(previous) => capture.max(previous + MIN_STEP),
        None => capture,
    };
    last.set(Some(capture));
    capture
}

/// The stream worker's end of a WebRTC session: hands each encoded access unit to the transport,
/// which packetizes, paces and sends it from its own thread. Dropping it closes the transport.
pub struct Sender {
    media: MediaHandle,
    last: Cell<Option<Instant>>,
}

impl Sender {
    pub fn new(media: MediaHandle) -> Self {
        Self {
            media,
            last: Cell::new(None),
        }
    }

    /// The client's address on the pair ICE selected, once it has answered.
    pub fn peer(&self) -> Option<SocketAddr> {
        self.media.peer()
    }

    /// Video datagrams the loss switch has thrown away so far; None while it is off.
    pub fn simulated_losses(&self) -> Option<u64> {
        self.media.simulated_losses()
    }

    /// Queue frames the encoder returned. A frame the transport cannot take is dropped and a
    /// keyframe requested by the transport; the stream goes on.
    pub fn submit(
        &self,
        session: &Session,
        frames: Vec<Encoded>,
        call_latency: Duration,
        processing_started: Instant,
        claims: &mut Claims,
    ) -> Result<()> {
        if frames.is_empty() {
            return Ok(());
        }
        if !self.media.is_running() {
            // Closing the transport stops the session first; anything else is a failure.
            ensure!(
                session.stopping(),
                "the WebRTC transport stopped without closing"
            );
            return Ok(());
        }
        let polled = Instant::now();
        for frame in frames {
            ensure!(
                matches!(frame.unit, Unit::Whole),
                "WebRTC takes whole access units, not slices"
            );
            let latency = frame.latency.unwrap_or(call_latency);
            let claimed = polled.checked_sub(latency).unwrap_or(polled);
            let claim = claims.take(frame.presentation);
            let capture = monotonic(&self.last, frame.presentation.unwrap_or(claimed));
            let bytes = frame.bytes.len() as u64;
            let submitted = self.media.submit_video(VideoFrame {
                data: frame.bytes,
                capture,
                keyframe: frame.idr,
            });
            let stats = &session.stats;
            match submitted {
                Submit::Queued => {
                    let sent_at = Instant::now();
                    stats.latency_us.store(micros(latency), Ordering::Relaxed);
                    stats.frames.fetch_add(1, Ordering::Relaxed);
                    stats.bytes.fetch_add(bytes, Ordering::Relaxed);
                    let age = claim.as_ref().map_or(0, |claim| {
                        micros(claim.claimed.saturating_duration_since(claim.presented))
                    });
                    stats.performance.lock().unwrap().record_timing(
                        sent_at,
                        Timing {
                            period: session.stream_mode().period(),
                            encode: micros(latency),
                            host: micros(polled.saturating_duration_since(processing_started)),
                            age,
                            sent: micros(sent_at.saturating_duration_since(claimed)),
                        },
                        bytes,
                    );
                }
                Submit::Dropped => {
                    stats.frames_replaced.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(bytes, keyframe = frame.idr, "WebRTC video frame dropped");
                }
            }
        }
        Ok(())
    }
}

/// What the actor learns from the transport.
enum Event {
    Connected,
}

/// The transport's hooks run on its threads and know the session only through this: the session
/// is set once it exists, which is after the transport started.
struct Bridge {
    session: Mutex<Option<Weak<Session>>>,
    /// The transport closed before the session was bound.
    closed: AtomicBool,
    /// The transport connected, which may be before the session was bound.
    connected: AtomicBool,
    events: UnboundedSender<Event>,
    /// The session's input worker, attached before the transport starts so no frame the client
    /// sends early is lost.
    input: Mutex<Option<InputHandle>>,
}

impl Bridge {
    fn new(events: UnboundedSender<Event>) -> Arc<Self> {
        Arc::new(Self {
            session: Mutex::new(None),
            closed: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            events,
            input: Mutex::new(None),
        })
    }

    fn attach_input(&self, input: InputHandle) {
        *self.input.lock().unwrap() = Some(input);
    }

    fn input(&self) -> Option<InputHandle> {
        self.input.lock().unwrap().clone()
    }

    fn bind(&self, session: &Arc<Session>) {
        *self.session.lock().unwrap() = Some(Arc::downgrade(session));
        if let Some(input) = self.input() {
            input.bind(session.clone());
        }
        if self.closed.load(Ordering::SeqCst) {
            session.stop();
        } else if self.connected.load(Ordering::SeqCst) {
            // Its keyframe request had no session to reach.
            session.request_host_keyframe();
        }
    }

    fn session(&self) -> Option<Arc<Session>> {
        self.session.lock().unwrap().as_ref()?.upgrade()
    }

    /// A PLI or FIR is the client's keyframe request, like Moonlight's; the transport's own
    /// reasons are the host's.
    fn idr(&self, reason: IdrReason) {
        let Some(session) = self.session() else {
            return;
        };
        match reason {
            IdrReason::Pli | IdrReason::Fir => session.request_client_keyframe(),
            IdrReason::Connected => session.request_host_keyframe(),
            IdrReason::Reconnected
            | IdrReason::QueueFull
            | IdrReason::PacerBacklog
            | IdrReason::WaitingForKeyframe
            | IdrReason::WriteFailed => session.request_idr(),
        }
    }

    fn event(&self, event: TransportEvent) {
        match event {
            TransportEvent::Connected => {
                self.connected.store(true, Ordering::SeqCst);
                let _ = self.events.send(Event::Connected);
            }
            TransportEvent::Closed => {
                self.closed.store(true, Ordering::SeqCst);
                tracing::info!("WebRTC transport closed; ending the session");
                if let Some(session) = self.session() {
                    session.stop();
                }
                // Releases what the client still holds.
                if let Some(input) = self.input() {
                    input.closed();
                }
            }
            TransportEvent::Disconnected => {
                tracing::warn!("WebRTC path down; the session ends if it does not come back")
            }
            TransportEvent::Reconnected => tracing::info!("WebRTC path restored"),
            TransportEvent::Control {
                data,
                binary: false,
            } => {
                match std::str::from_utf8(&data).map(classify_control) {
                    Ok(ControlMessage::KeyframeRequest) => {
                        if let Some(session) = self.session() {
                            session.request_client_keyframe();
                        }
                    }
                    // Reference-style clients announce their pads here; the
                    // snapshots that follow on `gamepad` are ignored without it.
                    Ok(ControlMessage::GamepadAnnouncement) => {
                        if let Some(input) = self.input() {
                            input.gamepad(data, false, Instant::now());
                        }
                    }
                    _ => {}
                }
            }
            TransportEvent::Gamepad { data, binary } => {
                if let Some(input) = self.input() {
                    input.gamepad(data, binary, Instant::now());
                }
            }
            TransportEvent::Input { data, binary } => {
                if let Some(input) = self.input() {
                    input.input(data, binary);
                }
            }
            TransportEvent::ChannelClosed { label } => {
                if let Some(input) = self.input() {
                    input.channel_closed(&label);
                }
            }
            TransportEvent::ChannelOpen(_) | TransportEvent::ChannelRejected(_) => {}
            TransportEvent::Control { .. } => {}
        }
    }

    fn hooks(self: &Arc<Self>) -> Hooks {
        let (idr, event) = (self.clone(), self.clone());
        Hooks::new(move |reason| idr.idr(reason), move |e| event.event(e))
    }
}

/// A request to start or switch to an app.
pub struct Request {
    pub app_id: String,
    pub video: VideoSettings,
    pub audio: AudioSettings,
}

pub enum Command {
    Start(Request),
    Switch(Request),
    Stop,
    StopHost,
    Answer { session_id: u64, sdp: String },
}

/// The connection's end of its session actor. Dropping it ends the actor and stops its session.
pub struct Commands(UnboundedSender<Command>);

impl Commands {
    pub fn send(&self, command: Command) -> bool {
        self.0.send(command).is_ok()
    }
}

/// Run the session actor of one signalling connection. Its replies arrive on `out`. `peer` is the
/// signalling peer and `local` the address it connected to, which the media socket binds.
pub fn spawn(
    h: Shared,
    media: Arc<Media>,
    client_id: String,
    peer: SocketAddr,
    local: IpAddr,
    out: UnboundedSender<ServerMessage>,
) -> Commands {
    let (commands, inbox) = unbounded_channel();
    let actor = Actor {
        h,
        media,
        client_id,
        peer,
        local,
        inbox,
        out,
        live: None,
    };
    tokio::spawn(actor.run());
    Commands(commands)
}

/// A session the actor started and has not reported as ended.
struct Live {
    id: u64,
    session: Arc<Session>,
    settings: StreamSettings,
    events: UnboundedReceiver<Event>,
    /// Keeps the event channel open for as long as the session is live.
    _bridge: Arc<Bridge>,
    connected: bool,
    /// The transport connected before the worker vouched for an HDR source: "streaming" waits.
    streaming_held: bool,
    since: Instant,
}

/// A checked request.
struct Checked {
    client: butterpollo_core::state::Client,
    settings: StreamSettings,
    app_id: u32,
}

/// What the actor does after a request.
enum Flow {
    Done,
    /// The request ended because another one arrived; run that next.
    Again(Command),
    /// The connection is gone.
    Closed,
}

/// What came of waiting for the answer.
enum Waited {
    Answer(String),
    Interrupted(Command),
    TimedOut,
    /// A newer launch replaced this one, which has no stream to wait for any more.
    Superseded,
    Closed,
}

struct Actor {
    h: Shared,
    media: Arc<Media>,
    client_id: String,
    peer: SocketAddr,
    local: IpAddr,
    inbox: UnboundedReceiver<Command>,
    out: UnboundedSender<ServerMessage>,
    live: Option<Live>,
}

/// Resolves with the live session's next event; never when there is none.
async fn next_event(live: &mut Option<Live>) -> Option<Event> {
    match live {
        Some(live) => live.events.recv().await,
        None => std::future::pending().await,
    }
}

fn running_app(h: &Shared) -> Option<u32> {
    h.current_app.lock().unwrap().as_ref().map(|app| app.id)
}

fn host_status(
    state: HostSessionState,
    running: Option<u32>,
    target: Option<u32>,
    message: Option<String>,
) -> ServerMessage {
    ServerMessage::HostSessionStatus {
        state,
        running_app_id: running.map(|id| id.to_string()),
        target_app_id: target.map(|id| id.to_string()),
        message,
    }
}

impl Actor {
    async fn run(mut self) {
        let mut watch = tokio::time::interval(WATCH);
        watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                command = self.inbox.recv() => match command {
                    Some(command) => {
                        if matches!(self.handle(command).await, Flow::Closed) {
                            break;
                        }
                    }
                    None => break,
                },
                Some(event) = next_event(&mut self.live) => self.on_event(event),
                _ = watch.tick() => self.watch(),
            }
        }
        // The signalling connection is gone: the stream goes with it, as with stop-session.
        if let Some(live) = self.live.take() {
            tracing::info!(
                session = live.id,
                "WebRTC signalling closed; stopping its session"
            );
            live.session.stop();
        }
    }

    fn say(&self, message: ServerMessage) {
        let _ = self.out.send(message);
    }

    fn refuse(&self, kind: &str, code: ErrorCode, message: impl Into<String>, host_op: bool) {
        let message = message.into();
        // Once the host operation began the client sees it end, not a request fail.
        self.say(if host_op {
            host_status(
                HostSessionState::Failed,
                running_app(&self.h),
                None,
                Some(message),
            )
        } else {
            ServerMessage::error(kind, code, message)
        });
    }

    async fn handle(&mut self, command: Command) -> Flow {
        let mut next = command;
        loop {
            let flow = match next {
                Command::Start(request) => self.start(request, "start-session").await,
                Command::Switch(request) => self.switch(request).await,
                Command::StopHost => self.stop_host().await,
                Command::Stop => {
                    self.stop();
                    Flow::Done
                }
                Command::Answer { session_id, .. } => {
                    tracing::debug!(session_id, "WebRTC answer without an offer ignored");
                    Flow::Done
                }
            };
            match flow {
                Flow::Again(command) => next = command,
                other => return other,
            }
        }
    }

    fn on_event(&mut self, event: Event) {
        let Some(live) = &mut self.live else {
            return;
        };
        match event {
            Event::Connected => {
                live.connected = true;
                // An HDR stream is announced only once the worker has seen the display in HDR,
                // so one that is refused ends with an error and was never "streaming".
                if live.settings.hdr && !live.session.source_checked.load(Ordering::Acquire) {
                    live.streaming_held = true;
                    return;
                }
                self.announce_streaming();
            }
        }
    }

    fn announce_streaming(&mut self) {
        let Some(live) = &mut self.live else {
            return;
        };
        live.streaming_held = false;
        let (video, audio) = live.settings.to_wire();
        let message = ServerMessage::SessionStatus {
            state: SessionState::Streaming,
            session_id: Some(live.id),
            video: Some(video),
            audio: Some(audio),
            message: None,
        };
        self.say(message);
    }

    /// Report a session that ended by itself, and end one that never connected.
    fn watch(&mut self) {
        let Some(live) = &self.live else {
            return;
        };
        if live.session.stopping() {
            let (id, failed) = (live.id, live.session.failed());
            self.live = None;
            self.say(if failed {
                ServerMessage::SessionStatus {
                    state: SessionState::Error,
                    session_id: Some(id),
                    video: None,
                    audio: None,
                    message: Some(
                        "The stream stopped because of an error on the host; the host log has the reason".into(),
                    ),
                }
            } else {
                ServerMessage::session_status(SessionState::Idle, Some(id))
            });
        } else if live.streaming_held && live.session.source_checked.load(Ordering::Acquire) {
            self.announce_streaming();
        } else if live.streaming_held && live.since.elapsed() > self.h.webrtc_links.held_limit() {
            tracing::warn!(
                session = live.id,
                "the stream worker did not check the HDR source in time"
            );
            live.session.fail();
        } else if !live.connected && live.since.elapsed() > CONNECT_TIMEOUT {
            tracing::warn!(session = live.id, "WebRTC session did not connect in time");
            live.session.fail();
        }
    }

    /// `stop-session`: the stream ends, the app keeps running.
    fn stop(&mut self) {
        match self.live.take() {
            Some(live) => {
                live.session.stop();
                self.say(ServerMessage::session_status(
                    SessionState::Stopping,
                    Some(live.id),
                ));
                self.say(ServerMessage::session_status(
                    SessionState::Idle,
                    Some(live.id),
                ));
            }
            None => self.say(ServerMessage::session_status(SessionState::Idle, None)),
        }
    }

    /// The request, if the device may make it and the host can serve it. Nothing is adjusted or
    /// substituted: a request outside the capabilities is `unsupported-settings`.
    fn check(&self, kind: &str, request: &Request) -> Result<Checked, Box<ServerMessage>> {
        let error =
            |kind: &str, code, message: &str| Box::new(ServerMessage::error(kind, code, message));
        let client = self
            .h
            .webrtc
            .verify(&self.h, &self.client_id)
            .map_err(|code| error(kind, code, "This device is not allowed to stream"))?;
        if !client.allows(LAUNCH) {
            return Err(error(
                kind,
                ErrorCode::NotAuthenticated,
                "This device may not start streams",
            ));
        }
        let codecs = self.h.codecs.load(Ordering::Acquire);
        let offer = server::frame_rate_offer(&self.h.config.read().unwrap());
        let capabilities = HostCapabilities::new(server::host_probe(&self.h), offer);
        let settings = capabilities
            .validate(&request.video, &request.audio)
            .map_err(|invalid| Box::new(invalid.into_message(kind)))?;
        // The codec, in the bit depth asked for, must be there now (the AMF probe found it), not
        // only in the protocol's table. AV1 and HDR are refused when it is not, never streamed
        // as something else.
        let unsupported = |message: &str| error(kind, ErrorCode::UnsupportedSettings, message);
        let codec = butterpollo_core::codec_support::Codec {
            id: match settings.codec {
                Codec::H264 => 0,
                Codec::Hevc => 1,
                Codec::Av1 => 2,
            },
            hdr: settings.hdr,
        };
        if codecs & codec.bit() == 0 {
            let detail = butterpollo_core::codec_support::refusal_for(
                &self.h.codec_missing.lock().unwrap(),
                codec.id,
                settings.hdr,
                false,
            );
            return Err(unsupported(&detail));
        }
        let app_id = request.app_id.parse::<u32>().map_err(|_| {
            error(
                kind,
                ErrorCode::InvalidApplication,
                "The application does not exist",
            )
        })?;
        Ok(Checked {
            client,
            settings,
            app_id,
        })
    }

    async fn start(&mut self, request: Request, kind: &'static str) -> Flow {
        self.watch();
        if self.live.is_some() {
            self.refuse(
                kind,
                ErrorCode::SessionActive,
                "This connection already has a session; stop it first",
                false,
            );
            return Flow::Done;
        }
        match self.check(kind, &request) {
            Ok(checked) => {
                self.say(ServerMessage::session_status(SessionState::Starting, None));
                self.start_checked(checked, kind, false).await
            }
            Err(message) => {
                self.say(*message);
                Flow::Done
            }
        }
    }

    /// `switch-session`: end the current stream, and the app unless it is the target, then start.
    async fn switch(&mut self, request: Request) -> Flow {
        const KIND: &str = "switch-session";
        let checked = match self.check(KIND, &request) {
            Ok(checked) => checked,
            Err(message) => {
                self.say(*message);
                return Flow::Done;
            }
        };
        let running = running_app(&self.h);
        let same = running == Some(checked.app_id);
        // Replacing the app ends it, which `cancel` allows its owner, or anyone with no remote
        // session active.
        if !same && nvhttp::terminate_refused(&self.h, &checked.client.uuid) {
            self.say(Self::not_the_owner(KIND));
            return Flow::Done;
        }
        self.say(host_status(
            if same {
                HostSessionState::Resuming
            } else {
                HostSessionState::Switching
            },
            running,
            Some(checked.app_id),
            None,
        ));
        let stopped = self
            .stop_streams((!same).then(|| checked.client.uuid.clone()))
            .await;
        if stopped == StopStreams::Refused {
            self.say(Self::not_the_owner(KIND));
            return Flow::Done;
        }
        if self.live.is_some() {
            self.stop();
        }
        if stopped == StopStreams::TimedOut {
            self.refuse(
                KIND,
                ErrorCode::InternalError,
                "The previous stream did not stop in time",
                true,
            );
            return Flow::Done;
        }
        self.say(host_status(
            HostSessionState::Starting,
            running_app(&self.h),
            Some(checked.app_id),
            None,
        ));
        self.start_checked(checked, KIND, true).await
    }

    /// `stop-host-session`: every stream and the app end.
    async fn stop_host(&mut self) -> Flow {
        const KIND: &str = "stop-host-session";
        let client = match self.h.webrtc.verify(&self.h, &self.client_id) {
            Ok(client) if client.allows(LAUNCH) => client,
            _ => {
                self.say(ServerMessage::error(
                    KIND,
                    ErrorCode::NotAuthenticated,
                    "This device may not stop the host session",
                ));
                return Flow::Done;
            }
        };
        let running = running_app(&self.h);
        let busy = running.is_some() || !self.h.sessions.lock().unwrap().idle();
        if !busy && self.live.is_none() {
            self.say(host_status(HostSessionState::Stopped, None, None, None));
            return Flow::Done;
        }
        // The same guard as `cancel`, before anything is announced or stopped.
        if nvhttp::terminate_refused(&self.h, &client.uuid) {
            self.say(Self::not_the_owner(KIND));
            return Flow::Done;
        }
        self.say(host_status(HostSessionState::Stopping, running, None, None));
        let stopped = self.stop_streams(Some(client.uuid)).await;
        if stopped == StopStreams::Refused {
            self.say(Self::not_the_owner(KIND));
            return Flow::Done;
        }
        if self.live.is_some() {
            self.stop();
        }
        if stopped == StopStreams::Stopped {
            self.say(host_status(HostSessionState::Stopped, running, None, None));
        } else {
            self.say(host_status(
                HostSessionState::Failed,
                running,
                None,
                Some("A stream did not stop in time".into()),
            ));
        }
        Flow::Done
    }

    fn not_the_owner(kind: &str) -> ServerMessage {
        ServerMessage::error(
            kind,
            // The device is authenticated; another device's game is what stands in the way.
            ErrorCode::SessionActive,
            "Another device's game is running",
        )
    }

    /// Stop every stream on the blocking pool, and the app for the client named.
    async fn stop_streams(&self, terminate_app_for: Option<String>) -> StopStreams {
        let h = self.h.clone();
        tokio::task::spawn_blocking(move || nvhttp::stop_streams(&h, terminate_app_for.as_deref()))
            .await
            .unwrap_or(StopStreams::TimedOut)
    }

    /// Cancel a launch no stream took, leaving its app running as an abandoned Moonlight launch
    /// does. On the blocking pool: the launch's resources are released as it drops.
    async fn cancel(&self, launch: Launch) {
        let h = self.h.clone();
        let _ = tokio::task::spawn_blocking(move || cancel_launch(&h, launch)).await;
    }

    async fn wait_for_answer(&mut self, session_id: u64, launch: &str) -> Waited {
        let deadline = tokio::time::Instant::now() + ANSWER_TIMEOUT;
        loop {
            let step = (tokio::time::Instant::now() + WATCH).min(deadline);
            match tokio::time::timeout_at(step, self.inbox.recv()).await {
                Err(_) if tokio::time::Instant::now() >= deadline => return Waited::TimedOut,
                // A newer start by this device took the launch; the socket must be free for it.
                Err(_) if !self.h.sessions.lock().unwrap().pending.contains_key(launch) => {
                    return Waited::Superseded;
                }
                Err(_) => {}
                Ok(None) => return Waited::Closed,
                Ok(Some(Command::Answer {
                    session_id: id,
                    sdp,
                })) if id == session_id => {
                    return Waited::Answer(sdp);
                }
                Ok(Some(Command::Answer { session_id: id, .. })) => {
                    tracing::debug!(session_id = id, "WebRTC answer for another session ignored");
                }
                Ok(Some(Command::Start(_))) => self.say(ServerMessage::error(
                    "start-session",
                    ErrorCode::SessionActive,
                    "A session is already starting",
                )),
                Ok(Some(other)) => return Waited::Interrupted(other),
            }
        }
    }

    async fn start_checked(&mut self, checked: Checked, kind: &'static str, host_op: bool) -> Flow {
        let Checked {
            client,
            settings,
            app_id,
        } = checked;
        let request = WebRtcLaunch {
            client,
            peer: self.peer.ip(),
            app_id,
            width: settings.width,
            height: settings.height,
            fps: settings.fps,
            hdr: settings.hdr,
        };
        let h = self.h.clone();
        let launched =
            tokio::task::spawn_blocking(move || nvhttp::launch_webrtc(&h, request)).await;
        let launch = match launched {
            Ok(Ok(launch)) => launch,
            Ok(Err(error)) => {
                let (code, message) = match error {
                    WebRtcLaunchError::InvalidApplication => (
                        ErrorCode::InvalidApplication,
                        "The application does not exist".to_owned(),
                    ),
                    WebRtcLaunchError::SessionActive(message) => {
                        (ErrorCode::SessionActive, message.to_owned())
                    }
                    WebRtcLaunchError::HostBusy => (
                        ErrorCode::HostOperationActive,
                        "The host is busy; try again shortly".to_owned(),
                    ),
                    WebRtcLaunchError::Failed(message) => (ErrorCode::InternalError, message),
                };
                tracing::warn!(%code, %message, "WebRTC launch refused");
                self.refuse(kind, code, message, host_op);
                return Flow::Done;
            }
            Err(error) => {
                tracing::error!(%error, "WebRTC launch panicked");
                self.refuse(kind, ErrorCode::InternalError, "The launch failed", host_op);
                return Flow::Done;
            }
        };
        let session_id = self.h.webrtc_links.next_session_id();
        let prepared = match self.prepare(&settings).await {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "WebRTC session could not be prepared");
                self.cancel(launch).await;
                self.refuse(
                    kind,
                    ErrorCode::InternalError,
                    format!("{error:#}"),
                    host_op,
                );
                return Flow::Done;
            }
        };
        self.say(ServerMessage::session_status(
            SessionState::StartingWebrtc,
            Some(session_id),
        ));
        self.say(ServerMessage::offer(session_id, prepared.offer_sdp()));
        // Every way out but an answer frees the media socket first: releasing the launch can
        // take a while, and the next start needs the port.
        let answer = match self.wait_for_answer(session_id, &launch.id).await {
            Waited::Answer(answer) => answer,
            Waited::Interrupted(command) => {
                drop(prepared);
                self.cancel(launch).await;
                return Flow::Again(command);
            }
            Waited::Superseded => {
                drop(prepared);
                self.cancel(launch).await;
                self.refuse(
                    kind,
                    ErrorCode::SessionActive,
                    "A newer start replaced this one",
                    host_op,
                );
                return Flow::Done;
            }
            Waited::TimedOut => {
                drop(prepared);
                self.cancel(launch).await;
                self.refuse(
                    kind,
                    ErrorCode::InternalError,
                    "The client did not answer the offer in time",
                    host_op,
                );
                return Flow::Done;
            }
            Waited::Closed => {
                drop(prepared);
                self.cancel(launch).await;
                return Flow::Closed;
            }
        };
        let (events, receiver) = unbounded_channel();
        let bridge = Bridge::new(events);
        // Routing, spawning the transport's threads and starting the session block.
        let (h, media, peer) = (self.h.clone(), self.media.clone(), self.peer);
        let live_bridge = bridge.clone();
        let started = tokio::task::spawn_blocking(move || {
            go_live(
                &h,
                &media,
                peer,
                launch,
                settings,
                prepared,
                &answer,
                &live_bridge,
            )
        })
        .await
        .unwrap_or_else(|error| {
            Err(Rejected::Host(anyhow!(
                "starting the session panicked: {error}"
            )))
        });
        match started {
            Ok(session) => {
                self.live = Some(Live {
                    id: session_id,
                    session,
                    settings,
                    events: receiver,
                    _bridge: bridge,
                    connected: false,
                    streaming_held: false,
                    since: Instant::now(),
                });
                Flow::Done
            }
            Err(rejected) => {
                let (code, error) = match rejected {
                    Rejected::Answer(error) => (ErrorCode::InvalidMessage, error),
                    Rejected::Host(error) => (ErrorCode::InternalError, error),
                };
                tracing::error!(error = %format!("{error:#}"), "WebRTC session could not start");
                self.refuse(kind, code, format!("{error:#}"), host_op);
                Flow::Done
            }
        }
    }

    /// Bind the media socket and build the offer.
    async fn prepare(&self, settings: &StreamSettings) -> Result<Prepared> {
        let port = self.h.config.read().unwrap().webrtc_media_port();
        // The previous session's worker frees the port as it ends, which can be moments after the
        // client was told the session is over.
        let deadline = tokio::time::Instant::now() + BIND_WAIT;
        let socket = loop {
            match media::bind_socket((self.local, port).into()) {
                Ok(socket) => break socket,
                Err(error)
                    if tokio::time::Instant::now() < deadline
                        && error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse) =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error),
            }
        };
        let local = socket.local_addr().context("reading the media address")?;
        let spec = OfferSpec {
            video: VideoMode {
                codec: match settings.codec {
                    Codec::H264 => VideoCodec::H264,
                    Codec::Hevc => VideoCodec::Hevc,
                    Codec::Av1 => VideoCodec::Av1,
                },
                width: settings.width,
                height: settings.height,
                fps: settings.fps,
                hdr: settings.hdr,
                bitrate_kbps: settings.bitrate_kbps,
            },
            audio: AudioMode::request(settings.audio_channels as usize)?,
            // NACK and RTX are compared with keyframe recovery alone in B5.
            rtx: self.h.config.read().unwrap().webrtc_rtx(),
        };
        let mut rtc = sdp::rtc_config(&spec)?.build(Instant::now());
        rtc.add_local_candidate(
            str0m::Candidate::host(local, "udp").context("the host candidate")?,
        );
        let offer = sdp::create_offer(&mut rtc, &spec)?;
        Ok(Prepared {
            socket: Some(socket),
            local,
            rtc: Some(rtc),
            offer: Some(offer),
            spec,
        })
    }
}

/// Release a launch no stream took, and its transport if nobody took that either.
fn cancel_launch(h: &Shared, launch: Launch) {
    h.webrtc_links.discard(&launch.id);
    let canceled = h.sessions.lock().unwrap().request_stop(Some(&launch.id));
    drop(canceled);
}

/// Accept the answer, start the transport and the stream worker. Blocking: it routes, starts the
/// transport's threads and takes the session lock. A launch that does not start is cancelled here,
/// where releasing what it holds may block too.
#[allow(clippy::too_many_arguments)]
fn go_live(
    h: &Shared,
    media: &Arc<Media>,
    peer: SocketAddr,
    launch: Launch,
    settings: StreamSettings,
    mut prepared: Prepared,
    answer: &str,
    bridge: &Arc<Bridge>,
) -> Result<Arc<Session>, Rejected> {
    let started = start_live(
        h,
        media,
        peer,
        &launch,
        &settings,
        &mut prepared,
        answer,
        bridge,
    );
    if started.is_err() {
        drop(prepared);
        cancel_launch(h, launch);
    }
    started
}

/// The Opus layout the audio thread encodes: the one the client answered with for 5.1 and 7.1,
/// stereo otherwise, at the bitrate [`Negotiated::webrtc`] took out of the video budget for it.
fn audio_layout(channels: u8, answered: Option<OpusLayout>) -> Result<OpusLayout> {
    let mut layout = match (channels, answered) {
        (2, _) => OpusLayout::select(2, false, None)?,
        (6 | 8, Some(layout)) if layout.channels == usize::from(channels) => layout,
        (6 | 8, _) => bail!("the answer has no {channels}-channel multiopus layout"),
        _ => bail!("unsupported audio channel count {channels}"),
    };
    layout.bitrate = webrtc_audio_kbps(channels) as i32 * 1000;
    Ok(layout)
}

#[allow(clippy::too_many_arguments)]
fn start_live(
    h: &Shared,
    media: &Arc<Media>,
    peer: SocketAddr,
    launch: &Launch,
    settings: &StreamSettings,
    prepared: &mut Prepared,
    answer: &str,
    bridge: &Arc<Bridge>,
) -> Result<Arc<Session>, Rejected> {
    let (Some(socket), Some(mut rtc), Some(offer)) = (
        prepared.socket.take(),
        prepared.rtc.take(),
        prepared.offer.take(),
    ) else {
        return Err(Rejected::Answer(anyhow!("the offer was already answered")));
    };
    let (ids, answered) =
        sdp::accept_answer(&mut rtc, offer, answer, &prepared.spec).map_err(Rejected::Answer)?;
    let audio_layout = audio_layout(settings.audio_channels as u8, answered.audio_layout)
        .map_err(Rejected::Answer)?;
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
    // Before the transport, so the client's first frame has somewhere to go.
    let input = input::spawn(h.clone()).map_err(Rejected::Host)?;
    bridge.attach_input(input.clone());
    let handle = MediaHandle::spawn(
        socket,
        prepared.local,
        media::Session {
            rtc,
            ids,
            spec: &prepared.spec,
            // The signalling connection authenticated the device.
            authenticated: Arc::new(AtomicBool::new(true)),
        },
        options,
        bridge.hooks(),
    )
    .map_err(Rejected::Host)?;
    input.control(handle.control_sender());
    h.webrtc_links
        .put(launch.id.clone(), handle.with_audio_layout(audio_layout));
    let negotiated = Negotiated::webrtc(
        settings.width,
        settings.height,
        settings.fps,
        settings.codec,
        settings.hdr,
        settings.bitrate_kbps,
        settings.audio_channels as u8,
    );
    let started = h.sessions.lock().unwrap().start(launch.clone(), negotiated);
    let session = started.map_err(|error| Rejected::Host(error.context("starting the session")))?;
    bridge.bind(&session);
    media.start(h.clone(), session.clone());
    Ok(session)
}

/// Why an answered session did not start.
enum Rejected {
    /// The answer was not one for this offer.
    Answer(anyhow::Error),
    /// The host could not start the transport or the session, which may have been replaced.
    Host(anyhow::Error),
}

/// The transport pieces between building the offer and the client's answer.
struct Prepared {
    socket: Option<UdpSocket>,
    local: SocketAddr,
    rtc: Option<str0m::Rtc>,
    offer: Option<sdp::Offer>,
    spec: OfferSpec,
}

impl Prepared {
    fn offer_sdp(&self) -> String {
        self.offer
            .as_ref()
            .map(|offer| offer.sdp.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use butterpollo_core::session::Role;

    #[test]
    fn the_audio_layout_is_the_answered_one_at_the_bitrate_the_video_budget_paid_for() {
        let answered = |channels| {
            let (streams, coupled, mapping): (i32, i32, &[u8]) = match channels {
                6 => (4, 2, &[0, 4, 1, 2, 3, 5]),
                _ => (5, 3, &[0, 6, 1, 2, 3, 4, 5, 7]),
            };
            OpusLayout {
                channels: usize::from(channels),
                streams,
                coupled,
                mapping: mapping.to_vec(),
                bitrate: 1,
            }
        };
        // Stereo ignores the answer; the surround layouts keep theirs.
        let stereo = audio_layout(2, None).unwrap();
        assert_eq!((stereo.channels, stereo.bitrate), (2, 128_000));
        for channels in [6u8, 8] {
            let layout = audio_layout(channels, Some(answered(channels))).unwrap();
            let want = answered(channels);
            assert_eq!(
                (layout.streams, layout.coupled, &layout.mapping),
                (want.streams, want.coupled, &want.mapping)
            );
            // GameStream's surround bitrates, and what `Negotiated::webrtc` subtracts.
            assert_eq!(
                layout.bitrate,
                [256_000, 450_000][usize::from(channels / 8)]
            );
            assert_eq!(layout.bitrate, webrtc_audio_kbps(channels) as i32 * 1000);
            assert_eq!(
                layout.bitrate,
                OpusLayout::select(usize::from(channels), false, None)
                    .unwrap()
                    .bitrate
            );
            // No answered layout, or one for another channel count, is refused.
            assert!(audio_layout(channels, None).is_err());
            assert!(
                audio_layout(channels, Some(answered(if channels == 6 { 8 } else { 6 }))).is_err()
            );
        }
        assert!(audio_layout(4, None).is_err());
    }

    fn bridge() -> (Arc<Bridge>, UnboundedReceiver<Event>) {
        let (events, received) = unbounded_channel();
        (Bridge::new(events), received)
    }

    fn session(f: &Fixture) -> Arc<Session> {
        let session = f.session(f.client(u32::MAX), Role::Stream);
        // A new session starts with its first keyframe pending.
        assert!(session.idr.swap(false, Ordering::AcqRel));
        session
    }

    fn requested(session: &Session) -> (bool, u64) {
        (
            session.idr.load(Ordering::Acquire),
            session.stats.idr_requests.load(Ordering::Acquire),
        )
    }

    #[test]
    fn session_ids_are_positive_and_never_repeat() {
        let links = Links::default();
        let ids: Vec<u64> = (0..4).map(|_| links.next_session_id()).collect();
        assert_eq!(ids, [1, 2, 3, 4]);
        // A launch nobody put a transport for has nothing to take or discard.
        assert!(links.take("nothing").is_none());
        links.discard("nothing");
        assert_eq!(links.len(), 0);
    }

    #[test]
    fn capture_instants_never_repeat_or_step_back() {
        let last = Cell::new(None);
        let base = Instant::now();
        let first = monotonic(&last, base);
        assert_eq!(first, base);
        // The same instant, and an earlier one, each move on by at least a tick.
        let repeated = monotonic(&last, base);
        assert!(repeated >= first + MIN_STEP);
        let earlier = monotonic(&last, base - Duration::from_millis(5));
        assert!(earlier >= repeated + MIN_STEP);
        // A later one is kept as it is.
        let later = base + Duration::from_secs(1);
        assert_eq!(monotonic(&last, later), later);
    }

    #[test]
    fn a_pli_or_fir_is_the_clients_keyframe_request_and_the_rest_are_the_hosts() {
        let f = Fixture::new();
        let session = session(&f);
        let (bridge, _events) = bridge();
        bridge.bind(&session);
        assert_eq!(requested(&session), (false, 0));
        for (n, reason) in [IdrReason::Pli, IdrReason::Fir].into_iter().enumerate() {
            session.idr.store(false, Ordering::Release);
            bridge.idr(reason);
            assert_eq!(requested(&session), (true, n as u64 + 1), "{reason:?}");
        }
        let before = requested(&session).1;
        for reason in [
            IdrReason::Connected,
            IdrReason::Reconnected,
            IdrReason::QueueFull,
            IdrReason::PacerBacklog,
            IdrReason::WaitingForKeyframe,
            IdrReason::WriteFailed,
        ] {
            session.idr.store(false, Ordering::Release);
            bridge.idr(reason);
            assert!(session.idr.load(Ordering::Acquire), "{reason:?}");
        }
        assert_eq!(requested(&session).1, before + 6);
    }

    #[test]
    fn a_request_before_the_session_exists_is_ignored() {
        let f = Fixture::new();
        let session = session(&f);
        let (bridge, _events) = bridge();
        bridge.idr(IdrReason::Pli);
        bridge.event(TransportEvent::Control {
            data: br#"{"type":"request-keyframe"}"#.to_vec(),
            binary: false,
        });
        assert_eq!(requested(&session), (false, 0));
    }

    #[test]
    fn the_control_channels_request_keyframe_asks_for_one_and_other_text_does_not() {
        let f = Fixture::new();
        let session = session(&f);
        let (bridge, _events) = bridge();
        bridge.bind(&session);
        for text in [&br#"{"type":"idr"}"#[..], b"not json", b"{}"] {
            bridge.event(TransportEvent::Control {
                data: text.to_vec(),
                binary: false,
            });
        }
        // Binary frames on the control channel are not the JSON protocol.
        bridge.event(TransportEvent::Control {
            data: br#"{"type":"request-keyframe"}"#.to_vec(),
            binary: true,
        });
        assert_eq!(requested(&session), (false, 0));
        bridge.event(TransportEvent::Control {
            data: br#"{"type":"request-keyframe"}"#.to_vec(),
            binary: false,
        });
        assert_eq!(requested(&session), (true, 1));
    }

    #[test]
    fn pad_announcements_on_the_control_channel_reach_the_gamepad_worker_and_state_does_not() {
        let f = Fixture::new();
        let session = session(&f);
        let (bridge, _events) = bridge();
        bridge.bind(&session);
        let (input, mut received) = InputHandle::probe();
        bridge.attach_input(input);
        let connected = br#"{"v":2,"type":"gamepad-connected","controllerId":0}"#;
        let gone = br#"{"v":2,"type":"gamepad-disconnected","controllerId":0}"#;
        let state = br#"{"v":2,"type":"gamepad-state","controllerId":0}"#;
        for (data, binary) in [
            (&connected[..], false),
            (&state[..], false),
            (&gone[..], false),
            (&connected[..], true),
        ] {
            bridge.event(TransportEvent::Control {
                data: data.to_vec(),
                binary,
            });
        }
        let texts = received();
        assert_eq!(
            texts,
            [
                String::from_utf8_lossy(connected).into_owned(),
                String::from_utf8_lossy(gone).into_owned()
            ]
        );
        assert_eq!(requested(&session), (false, 0));
    }

    #[test]
    fn a_transport_that_closes_ends_the_session_even_before_it_is_bound() {
        let f = Fixture::new();
        let (bound, _events) = bridge();
        let session = session(&f);
        bound.bind(&session);
        bound.event(TransportEvent::Closed);
        assert!(session.stopping());

        // Closed first: the session is stopped the moment it is bound.
        let (early, _events) = bridge();
        let late = Fixture::new();
        let session = self::session(&late);
        early.event(TransportEvent::Closed);
        assert!(!session.stopping());
        early.bind(&session);
        assert!(session.stopping());
    }

    #[test]
    fn a_transport_that_connected_before_the_session_gets_its_keyframe_when_bound() {
        let f = Fixture::new();
        let session = session(&f);
        let (bridge, mut events) = bridge();
        bridge.event(TransportEvent::Connected);
        assert!(matches!(events.try_recv(), Ok(Event::Connected)));
        assert_eq!(requested(&session), (false, 0));
        bridge.bind(&session);
        assert_eq!(requested(&session), (true, 1));
        // A path that drops and returns changes nothing by itself.
        bridge.event(TransportEvent::Disconnected);
        bridge.event(TransportEvent::Reconnected);
        assert!(!session.stopping());
    }
}
