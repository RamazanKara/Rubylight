//! Per-connection WebRTC session actor: request validation, launch orchestration and lifecycle.

use super::{
    ANSWER_TIMEOUT, Bridge, CONNECT_TIMEOUT, Command, Event, LAUNCH, Request, WATCH,
    live::{Rejected, cancel_launch, go_live, prepare},
};
use crate::{guests::OwnerGuard, server};
use anyhow::anyhow;
use butterpollo_session::webrtc::{
    capabilities::{Codec, HostCapabilities, StreamSettings},
    protocol::{ErrorCode, HostSessionState, ServerMessage, SessionState},
};
use butterpollo_stream::{
    nvhttp::{self, StopStreams, WebRtcLaunch, WebRtcLaunchError},
    state::{Launch, Session, Shared},
    stream::Media,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{Arc, atomic::Ordering},
    time::Instant,
};
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, unbounded_channel};

/// A session the actor started and has not reported as ended.
pub(super) struct Live {
    id: u64,
    session: Arc<Session>,
    settings: StreamSettings,
    events: UnboundedReceiver<Event>,
    /// Keeps the event channel open for as long as the session is live.
    _bridge: Arc<Bridge>,
    /// A console stream's guests watch it while it is live (docs/browser-guests.md).
    _owner: Option<OwnerGuard>,
    connected: bool,
    /// The transport connected before the worker vouched for an HDR source: "streaming" waits.
    streaming_held: bool,
    since: Instant,
}

/// A checked request.
struct Checked {
    client: butterpollo_session::state::Client,
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

pub(super) struct Actor {
    pub(super) h: Shared,
    pub(super) media: Arc<Media>,
    pub(super) client_id: String,
    pub(super) peer: SocketAddr,
    pub(super) local: IpAddr,
    pub(super) this_pc: bool,
    pub(super) inbox: Receiver<Command>,
    pub(super) out: Sender<ServerMessage>,
    pub(super) live: Option<Live>,
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
    pub(super) async fn run(mut self) {
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
        if self.out.try_send(message).is_err() {
            tracing::debug!(
                client = %self.client_id,
                "WebRTC signalling outbox full or closed"
            );
        }
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
            live.session
                .fail("the stream worker did not check the HDR source in time");
        } else if !live.connected && live.since.elapsed() > CONNECT_TIMEOUT {
            tracing::warn!(session = live.id, "WebRTC session did not connect in time");
            live.session
                .fail("the WebRTC session did not connect in time");
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
        let client = crate::WebRtcRuntime::for_host(&self.h)
            .clients
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
        let codec = butterpollo_media_policy::codec_support::Codec {
            id: match settings.codec {
                Codec::H264 => 0,
                Codec::Hevc => 1,
                Codec::Av1 => 2,
            },
            hdr: settings.hdr,
            yuv444: false,
        };
        if codecs & codec.bit() == 0 {
            let detail = butterpollo_media_policy::codec_support::refusal_for(
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
        // Replacing or taking over a different running app must honor owner/remote-session/Continue Here
        // guards and WebRTC exclusivity before stopping any stream.
        if !same && nvhttp::terminate_refused(&self.h, &checked.client.uuid) {
            self.say(Self::not_the_owner(KIND));
            return Flow::Done;
        }
        if same
            && self.h.sessions.lock().unwrap().held_by_other(
                butterpollo_session::session::Role::Stream,
                butterpollo_session::session::Transport::WebRtc,
                &checked.client.uuid,
            )
        {
            self.say(ServerMessage::error(
                KIND,
                ErrorCode::SessionActive,
                "Another device is streaming over WebRTC",
            ));
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
        let stopped = self.stop_streams(checked.client.uuid.clone(), !same).await;
        if stopped == StopStreams::Refused {
            self.refuse(
                KIND,
                ErrorCode::SessionActive,
                "Another device's game is running",
                true,
            );
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
        let client = match crate::WebRtcRuntime::for_host(&self.h)
            .clients
            .verify(&self.h, &self.client_id)
        {
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
        let stopped = self.stop_streams(client.uuid, true).await;
        if stopped == StopStreams::Refused {
            self.refuse(
                KIND,
                ErrorCode::SessionActive,
                "Another device's game is running",
                true,
            );
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

    /// Stop host streams on the blocking pool, and the app when `stop_app` is true.
    async fn stop_streams(&self, client: String, stop_app: bool) -> StopStreams {
        let h = self.h.clone();
        tokio::task::spawn_blocking(move || nvhttp::stop_streams_for(&h, &client, stop_app))
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
            this_pc: self.this_pc,
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
                        (ErrorCode::SessionActive, message)
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
        let prepared = match prepare(
            &self.h,
            self.local,
            self.media.bind(),
            self.peer,
            self.this_pc,
            &settings,
        )
        .await
        {
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
            Ok((session, owner)) => {
                self.live = Some(Live {
                    id: session_id,
                    session,
                    settings,
                    events: receiver,
                    _bridge: bridge,
                    _owner: owner,
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
}
