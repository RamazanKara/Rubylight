//! Input from a WebRTC client (`docs/webrtc-design.md`): the `gamepad` and `input` DataChannels
//! reach the same injector the GameStream control path drives, and rumble and gamepad mouse mode
//! go back to the client on the `control` channel.
//!
//! One worker thread per session owns everything stateful: the core's gamepad state machine and
//! `input` channel parser, the injector, and the permission check. The transport's hooks run on
//! the media thread and must return quickly, so they only queue a [`Message`]. Nothing is
//! batched or coalesced: a message is injected as soon as the worker wakes for it.
//!
//! The worker sleeps on its queue while the session is idle and wakes on a deadline only for
//! work that has one: the 50 ms pointer tick while a controller is in mouse mode, the
//! injector's own key repeat and delayed release, and the 8 ms feedback poll while a controller
//! is plugged in. Whatever the client still holds when the `input` channel closes, the stream
//! stops or the session is replaced is released first, through core's `HeldInput` tracker.
//!
//! The injector sits behind [`Sink`] so the tests never reach `SendInput` or a virtual pad on
//! the desktop they run on.

use super::media::ControlSender;
use crate::{
    state::{Session, Shared},
    stream::{RUMBLE_DATA, RUMBLE_TRIGGER_DATA, effective_config, feedback_packets},
};
use anyhow::{Context, Result};
use butterpollo_core::{
    input::Input,
    session::EVENT_PERIOD,
    webrtc::{
        gamepad::{Action, Control, Gamepads},
        input::InputChannel,
    },
};
use butterpollo_windows::input::{Injector, PadReport};
use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// How often the injector's display lookup and pointer refresh run while input flows (the
/// GameStream control loop runs them every pass of its 8 ms poll).
const REFRESH: Duration = Duration::from_millis(8);
/// An injector call slower than this is logged, with the kinds of event in it.
const SLOW_APPLY: Duration = Duration::from_millis(2);
/// How often the injector is polled for rumble while a controller is plugged in, as the
/// GameStream control loop does.
const FEEDBACK_POLL: Duration = Duration::from_millis(8);
/// How often a worker that holds something looks at whether the session is stopping, in case the
/// transport takes a moment to report it closed.
const STOP_POLL: Duration = Duration::from_millis(100);
/// A failed injector is made again no sooner than this.
const RETRY: Duration = Duration::from_millis(500);

/// What the worker is told. Text frames only: the channels carry JSON.
enum Message {
    Gamepad(String, Instant),
    Input(String, Instant),
    /// A DataChannel closed.
    GamepadClosed,
    InputClosed,
    Bind(Arc<Session>),
    Control(ControlSender),
    /// The transport is gone.
    Closed,
}

/// The transport's end of a session's input worker. Every call only queues; none blocks.
#[derive(Clone)]
pub struct InputHandle(mpsc::Sender<Message>);

impl InputHandle {
    /// A handle with no worker behind it, and the texts it was given for the `gamepad` channel.
    #[cfg(test)]
    pub(crate) fn probe() -> (Self, impl FnMut() -> Vec<String>) {
        let (tx, rx) = mpsc::channel();
        let drain = move || {
            rx.try_iter()
                .filter_map(|m| match m {
                    Message::Gamepad(text, _) => Some(text),
                    _ => None,
                })
                .collect()
        };
        (Self(tx), drain)
    }

    /// A text frame from the `gamepad` channel, received at `now`. A binary or non-UTF-8 frame is
    /// not part of the protocol and is dropped with a warning.
    pub fn gamepad(&self, data: Vec<u8>, binary: bool, now: Instant) {
        if let Some(text) = text(data, binary, "gamepad") {
            let _ = self.0.send(Message::Gamepad(text, now));
        }
    }

    /// A text frame from the `input` channel.
    pub fn input(&self, data: Vec<u8>, binary: bool) {
        if let Some(text) = text(data, binary, "input") {
            let _ = self.0.send(Message::Input(text, Instant::now()));
        }
    }

    /// A DataChannel closed: `input` releases what the client held and `gamepad` unplugs its
    /// controllers.
    pub fn channel_closed(&self, label: &str) {
        match label {
            "input" => {
                let _ = self.0.send(Message::InputClosed);
            }
            "gamepad" => {
                let _ = self.0.send(Message::GamepadClosed);
            }
            _ => {}
        }
    }

    /// The session exists; its client's permissions and display apply from here.
    pub fn bind(&self, session: Arc<Session>) {
        let _ = self.0.send(Message::Bind(session));
    }

    /// The `control` channel to answer on.
    pub fn control(&self, sender: ControlSender) {
        let _ = self.0.send(Message::Control(sender));
    }

    /// The transport closed: release everything and end.
    pub fn closed(&self) {
        let _ = self.0.send(Message::Closed);
    }
}

/// An event of the kind a gamepad message produces, to ask what permission it needs.
fn controller_event() -> Input {
    Input::Controller {
        id: 0,
        active: 0,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

fn text(data: Vec<u8>, binary: bool, channel: &str) -> Option<String> {
    if binary {
        tracing::warn!(
            channel,
            "WebRTC input: a binary frame is not part of the protocol; dropped"
        );
        return None;
    }
    match String::from_utf8(data) {
        Ok(text) => Some(text),
        Err(_) => {
            tracing::warn!(channel, "WebRTC input: a frame that is not UTF-8; dropped");
            None
        }
    }
}

/// Where input events go, which is the injector except in tests.
pub trait Sink {
    /// Inject in order; returns why events failed.
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error>;
    /// When key repeat or a delayed release is next due.
    fn next_due(&self) -> Option<Instant>;
    fn due(&mut self) -> Result<()>;
    fn refresh(&mut self) -> Result<()>;
    /// The stream's display and size, which absolute input follows.
    fn geometry(&mut self, output: &str, width: u32, height: u32);
    /// New rumble, lights and trigger effects the controllers reported: (slot, kind, data).
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)>;
    fn feedback_allowed(&self, kind: u16) -> bool;
}

/// The Windows injector, made the way the GameStream control loop makes its own.
struct NativeSink(Injector);

impl NativeSink {
    fn create(h: &Shared, s: &Session) -> Result<Box<dyn Sink>> {
        let warnings = &s.launch.warnings;
        let c = effective_config(h, &s.launch).inspect_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "input configuration unavailable");
        })?;
        let output = s.output.read().unwrap().clone();
        let pads = h
            .controller_sessions
            .acquire(&s.launch, &c)
            .inspect_err(|error| {
                warnings.set(
                    "input_initialization",
                    format!("Controller recovery is still in progress ({error:#}); retrying."),
                );
            })?;
        let mut injector = Injector::new_options_reported(
            if output.is_empty() {
                c.get("output_name", "")
            } else {
                &output
            },
            c.get("gamepad", "auto"),
            &c,
            warnings.clone(),
        )
        .inspect_err(|error| {
            warnings.set(
                "input_initialization",
                format!("Input initialization failed ({error:#}); controls are unavailable. Check Input settings and the selected display, then reconnect."),
            );
        })?;
        injector.use_gamepads(pads);
        warnings.clear("input_initialization");
        let mode = s.stream_mode();
        injector.set_stream_size(mode.width, mode.height);
        Ok(Box::new(Self(injector)))
    }
}

impl Sink for NativeSink {
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        self.0.apply_all(events)
    }
    fn next_due(&self) -> Option<Instant> {
        self.0.next_due()
    }
    fn due(&mut self) -> Result<()> {
        self.0.due()
    }
    fn refresh(&mut self) -> Result<()> {
        self.0.refresh()
    }
    fn geometry(&mut self, output: &str, width: u32, height: u32) {
        self.0.set_output(output);
        self.0.set_stream_size(width, height);
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        self.0
            .gamepad_reports()
            .filter_map(|report| match report {
                PadReport::Feedback { id, kind, data } => Some((id, kind, data)),
                _ => None,
            })
            .collect()
    }
    fn feedback_allowed(&self, kind: u16) -> bool {
        self.0.feedback_allowed(kind)
    }
}

impl Drop for NativeSink {
    fn drop(&mut self) {
        // Unplugs the virtual pads asynchronously; the injector's own drop releases pointers.
        self.0.detach_gamepads();
    }
}

type Make = Box<dyn FnMut(&Session) -> Result<Box<dyn Sink>> + Send>;
type Reply = Box<dyn FnMut(Vec<u8>) -> bool + Send>;

/// What a session's input did, for its end-of-stream log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub gamepad_messages: u64,
    /// Valid gamepad messages that changed nothing: stale snapshots and unknown controllers.
    pub gamepad_ignored: u64,
    pub input_messages: u64,
    /// Messages the parser refused.
    pub invalid: u64,
    /// Key codes the table has no virtual key for.
    pub unknown_codes: u64,
    /// Events the injector was handed.
    pub injected: u64,
    /// Events the device's permissions or a missing injector kept from the desktop.
    pub dropped: u64,
    pub rumble_sent: u64,
    /// Messages whose events reached the injector, and how long the host held them: from the
    /// transport receiving the frame to the worker calling the injector. The injector call itself,
    /// `SendInput`, is below: Windows runs every low-level hook before it returns, so a desktop
    /// with a slow one (or a click that takes the focus) holds the call for milliseconds, as it
    /// does for ENet, and that is not the host's queue.
    pub queued: u64,
    pub queue_max_us: u64,
    pub queue_p99_us: u64,
    /// The parts of that delay: waiting in the channel for the worker, and the injector's
    /// periodic refresh that runs before the events; then the injector call itself.
    pub wait_max_us: u64,
    pub wait_p99_us: u64,
    pub refresh_max_us: u64,
    pub refresh_p99_us: u64,
    pub apply_max_us: u64,
    pub apply_p99_us: u64,
}

/// Sub-buckets per power of two in [`Delays`]: a percentile is at most 1/SUB too high.
const SUB: usize = 4;
/// Values below this are their own bucket.
const EXACT: u64 = 8;
const BUCKETS: usize = EXACT as usize + SUB * 37;

/// A histogram of delays in microseconds, four buckets to the octave: no allocation per message,
/// exact below 8 us and within 25 % above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Delays {
    count: u64,
    max_us: u64,
    buckets: [u64; BUCKETS],
}

impl Default for Delays {
    fn default() -> Self {
        Self {
            count: 0,
            max_us: 0,
            buckets: [0; BUCKETS],
        }
    }
}

impl Delays {
    fn bucket(us: u64) -> usize {
        if us < EXACT {
            return us as usize;
        }
        let octave = (u64::BITS - 1 - us.leading_zeros()) as usize;
        let sub = ((us >> (octave - 2)) & (SUB as u64 - 1)) as usize;
        (EXACT as usize + (octave - 3) * SUB + sub).min(BUCKETS - 1)
    }

    /// The largest value that falls in `bucket`.
    fn upper(bucket: usize) -> u64 {
        if bucket < EXACT as usize {
            return bucket as u64;
        }
        let octave = (bucket - EXACT as usize) / SUB + 3;
        let sub = (bucket - EXACT as usize) % SUB;
        (((SUB + sub + 1) as u64) << (octave - 2)) - 1
    }

    fn record(&mut self, delay: Duration) {
        let us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
        self.buckets[Self::bucket(us)] += 1;
        self.count += 1;
        self.max_us = self.max_us.max(us);
    }

    /// The upper edge of the bucket the 99th percentile falls in, never above the maximum.
    fn p99_us(&self) -> u64 {
        let rank = (self.count * 99).div_ceil(100);
        let mut seen = 0;
        for (bucket, count) in self.buckets.iter().enumerate() {
            seen += count;
            if self.count > 0 && seen >= rank {
                return Self::upper(bucket).min(self.max_us);
            }
        }
        0
    }
}

/// The state of one session's input, driven by [`run`] and by the tests with a clock of their own.
struct Core {
    session: Arc<Session>,
    pads: Gamepads,
    channel: InputChannel,
    sink: Option<Box<dyn Sink>>,
    make: Make,
    control: Option<Reply>,
    /// Permissions the device lacked and was warned about.
    denied: u32,
    retry_at: Option<Instant>,
    refresh_at: Instant,
    feedback_at: Instant,
    summary: Summary,
    delays: Delays,
    waits: Delays,
    refreshes: Delays,
    applies: Delays,
    /// When the worker took the frame being handled off its queue.
    dequeued: Option<Instant>,
    slow_logged: u32,
    /// When the frame being handled reached the transport, for the delay it waited in the queue.
    received: Option<Instant>,
    warned_invalid: bool,
}

impl Core {
    fn new(session: Arc<Session>, make: Make, now: Instant) -> Self {
        let mode = session.stream_mode();
        let mut core = Self {
            channel: InputChannel::new(mode.width, mode.height),
            session,
            pads: Gamepads::new(),
            sink: None,
            make,
            control: None,
            denied: 0,
            retry_at: None,
            refresh_at: now,
            feedback_at: now,
            summary: Summary::default(),
            delays: Delays::default(),
            waits: Delays::default(),
            refreshes: Delays::default(),
            applies: Delays::default(),
            dequeued: None,
            slow_logged: 0,
            received: None,
            warned_invalid: false,
        };
        // Made before input comes, as the GameStream control loop does: making it with the
        // first input held that input up.
        core.ensure_sink(now);
        core
    }

    fn summary(&self) -> Summary {
        Summary {
            unknown_codes: self.channel.unknown_codes(),
            queued: self.delays.count,
            queue_max_us: self.delays.max_us,
            queue_p99_us: self.delays.p99_us(),
            wait_max_us: self.waits.max_us,
            wait_p99_us: self.waits.p99_us(),
            refresh_max_us: self.refreshes.max_us,
            refresh_p99_us: self.refreshes.p99_us(),
            apply_max_us: self.applies.max_us,
            apply_p99_us: self.applies.p99_us(),
            ..self.summary
        }
    }

    fn ensure_sink(&mut self, now: Instant) -> bool {
        if self.sink.is_some() {
            return true;
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return false;
        }
        match (self.make)(&self.session) {
            Ok(sink) => {
                self.retry_at = None;
                self.sink = Some(sink);
                true
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "WebRTC input: the injector could not be made");
                self.retry_at = Some(now + RETRY);
                false
            }
        }
    }

    /// One `gamepad` channel message received at `now`. A device without the controller
    /// permission gets no slot, no mouse mode and no timers: the state machine never sees it.
    fn gamepad(&mut self, text: &str, now: Instant) {
        self.summary.gamepad_messages += 1;
        if !self.allowed(&controller_event()) {
            self.summary.dropped += 1;
            return;
        }
        self.received = Some(now);
        match self.pads.handle_message(text, now) {
            Ok(outcome) => {
                if !outcome.accepted && outcome.actions.is_empty() {
                    self.summary.gamepad_ignored += 1;
                }
                self.actions(outcome.actions, now);
            }
            Err(error) => self.invalid("gamepad", &error),
        }
        self.received = None;
    }

    /// One `input` channel message received at `now`.
    fn input(&mut self, text: &str, now: Instant) {
        self.summary.input_messages += 1;
        self.received = Some(now);
        match butterpollo_core::webrtc::input::parse(text) {
            Ok(message) => {
                let events = self.channel.translate(message);
                self.dispatch(events, now);
            }
            Err(error) => self.invalid("input", &error),
        }
        self.received = None;
    }

    /// Whether the device may send `event`; says once per kind in the stream's warnings when not.
    fn allowed(&mut self, event: &Input) -> bool {
        let client = &self.session.launch.client;
        let permission = event.required_permission();
        if client.allows(permission) {
            return true;
        }
        if self.denied & permission == 0 {
            self.denied |= permission;
            let (code, message) = event.denied_warning(&client.name);
            self.session.launch.warnings.set(&code, message);
        }
        false
    }

    fn invalid(&mut self, channel: &str, error: &dyn std::fmt::Display) {
        self.summary.invalid += 1;
        // The first is loud; a client that keeps sending them would flood the log.
        if !std::mem::replace(&mut self.warned_invalid, true) {
            tracing::warn!(channel, %error, "WebRTC input: invalid message dropped");
        } else {
            tracing::debug!(channel, %error, "WebRTC input: invalid message dropped");
        }
    }

    /// What the gamepad state machine decided: events for the injector and messages for the client.
    fn actions(&mut self, actions: Vec<Action>, now: Instant) {
        let mut events = Vec::new();
        for action in actions {
            match action {
                Action::Input(input) => events.push(input),
                Action::Control(control) => self.send(&control),
            }
        }
        self.dispatch(events, now);
    }

    fn send(&mut self, control: &Control) {
        let Some(send) = &mut self.control else {
            tracing::debug!("WebRTC input: no control channel to answer on");
            return;
        };
        if matches!(control, Control::Rumble { .. }) {
            self.summary.rumble_sent += 1;
        }
        send(control.to_json().into_bytes());
    }

    /// Check each event against the device's permissions, as the GameStream control loop does,
    /// remember what it holds, and hand the rest to the injector.
    fn dispatch(&mut self, events: Vec<Input>, now: Instant) {
        let mut allowed = Vec::with_capacity(events.len());
        for event in events {
            if !self.allowed(&event) {
                self.summary.dropped += 1;
                continue;
            }
            self.channel.observe(&event);
            allowed.push(event);
        }
        self.inject(allowed, now);
    }

    fn inject(&mut self, events: Vec<Input>, now: Instant) {
        if events.is_empty() {
            return;
        }
        if !self.ensure_sink(now) {
            self.summary.dropped += events.len() as u64;
            return;
        }
        let session = &self.session;
        let sink = self.sink.as_mut().expect("the injector was just made");
        // The stream's display can be created or renamed after input began.
        let mode = session.stream_mode();
        sink.geometry(&session.output.read().unwrap(), mode.width, mode.height);
        let mut refreshed = Duration::ZERO;
        if now >= self.refresh_at {
            self.refresh_at = now + REFRESH;
            let started = Instant::now();
            if let Err(error) = sink.refresh() {
                session.launch.warnings.event("input_pointer", format!("Pointer refresh failed ({error:#}); pointer updates may be missing. Unlock the desktop or reconnect."), EVENT_PERIOD);
            }
            refreshed = started.elapsed();
        }
        self.summary.injected += events.len() as u64;
        let started = Instant::now();
        for error in sink.apply(&events) {
            session.launch.warnings.event("input_injection", format!("Input injection failed ({error:#}); keyboard, mouse or controller actions may be missing. Unlock the desktop and check Windows input permissions."), EVENT_PERIOD);
        }
        let done = Instant::now();
        if done.saturating_duration_since(started) >= SLOW_APPLY && self.slow_logged < 16 {
            self.slow_logged += 1;
            let kinds: Vec<String> = events
                .iter()
                .map(|e| {
                    format!("{e:?}")
                        .split([' ', '(', '{'])
                        .next()
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
            tracing::info!(
                took_us = done.saturating_duration_since(started).as_micros() as u64,
                ?kinds,
                "WebRTC input: a slow injection"
            );
        }
        if let Some(received) = self.received {
            self.delays
                .record(started.saturating_duration_since(received));
            self.refreshes.record(refreshed);
            self.applies.record(done.saturating_duration_since(started));
            if let Some(dequeued) = self.dequeued {
                self.waits
                    .record(dequeued.saturating_duration_since(received));
            }
        }
    }

    /// Time-driven work that is due: the pointer tick of mouse mode, the injector's key repeat
    /// and delayed release, and rumble. False when the session is stopping.
    fn poll(&mut self, now: Instant) -> bool {
        if self.session.stopping() {
            return false;
        }
        let actions = self.pads.tick(now);
        self.actions(actions, now);
        if let Some(sink) = &mut self.sink
            && sink.next_due().is_some_and(|due| now >= due)
            && let Err(error) = sink.due()
        {
            self.session.launch.warnings.event("input_repeat", format!("Input release or repeat failed ({error:#}); held controls may not update. Unlock the desktop or reconnect."), EVENT_PERIOD);
        }
        if self.pads.active_mask() != 0 && now >= self.feedback_at {
            self.feedback_at = now + FEEDBACK_POLL;
            self.feedback();
        }
        true
    }

    /// Rumble the controllers' drivers posted, as `rumble` messages.
    fn feedback(&mut self) {
        let Some(sink) = &mut self.sink else {
            return;
        };
        let mut controls = Vec::new();
        for (slot, kind, data) in sink.feedback() {
            for (kind, payload) in feedback_packets(slot, kind, &data) {
                if !sink.feedback_allowed(kind) {
                    continue;
                }
                let word = |at: usize| u16::from_le_bytes([payload[at], payload[at + 1]]);
                // The payloads are the GameStream ones: marker, slot, low, high; and slot, left,
                // right.
                let control = match (kind, payload.len()) {
                    (RUMBLE_DATA, 10) => self.pads.rumble(slot, word(6), word(8)),
                    (RUMBLE_TRIGGER_DATA, 6) => self.pads.trigger_rumble(slot, word(2), word(4)),
                    _ => None,
                };
                controls.extend(control);
            }
        }
        for control in controls {
            self.send(&control);
        }
    }

    /// How long the worker may sleep: None to wait for the next message.
    fn wait(&self, now: Instant) -> Option<Duration> {
        // An injector, a plugged pad or a held key is something to give up when the session
        // stops, even if the transport is slow to say so.
        let busy =
            self.sink.is_some() || self.pads.active_mask() != 0 || !self.channel.held().is_empty();
        let deadlines = [
            self.pads.next_tick(),
            self.sink.as_ref().and_then(|sink| sink.next_due()),
            (self.pads.active_mask() != 0).then_some(self.feedback_at),
            busy.then_some(now + STOP_POLL),
        ];
        deadlines
            .into_iter()
            .flatten()
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// The `input` channel closed: let go of every key and button the client holds.
    fn release_held(&mut self, now: Instant) {
        let events = self.channel.release_all();
        if !events.is_empty() {
            tracing::info!(
                count = events.len(),
                "WebRTC input: releasing what the client held"
            );
            self.inject(events, now);
        }
    }

    /// The `gamepad` channel closed: unplug every controller, which also releases its mouse mode.
    fn unplug(&mut self, now: Instant) {
        let actions = self.pads.close();
        self.actions(actions, now);
    }

    /// Release everything and report. Safe to call twice.
    fn finish(&mut self, now: Instant) {
        self.unplug(now);
        self.release_held(now);
        // Drops the injector, which unplugs the virtual pads.
        self.sink = None;
        let summary = self.summary();
        tracing::info!(
            client = %self.session.launch.client.name,
            gamepad_messages = summary.gamepad_messages,
            gamepad_ignored = summary.gamepad_ignored,
            input_messages = summary.input_messages,
            invalid = summary.invalid,
            unknown_codes = summary.unknown_codes,
            injected = summary.injected,
            dropped = summary.dropped,
            rumble_sent = summary.rumble_sent,
            queued = summary.queued,
            queue_max_us = summary.queue_max_us,
            queue_p99_us = summary.queue_p99_us,
            wait_max_us = summary.wait_max_us,
            wait_p99_us = summary.wait_p99_us,
            refresh_max_us = summary.refresh_max_us,
            refresh_p99_us = summary.refresh_p99_us,
            apply_max_us = summary.apply_max_us,
            apply_p99_us = summary.apply_p99_us,
            "WebRTC input summary"
        );
    }
}

/// Start the input worker of a session. It idles until [`InputHandle::bind`] gives it the
/// session; anything the client sends before that waits in its queue.
pub fn spawn(h: Shared) -> Result<InputHandle> {
    let (tx, rx) = mpsc::channel();
    let make: Make = Box::new(move |session| {
        #[cfg(test)]
        if let Some(fixture) = &h.reconnect_fixture {
            return fixture.webrtc_input.make();
        }
        NativeSink::create(&h, session)
    });
    std::thread::Builder::new()
        .name("webrtc-input".into())
        .spawn(move || run(rx, make))
        .context("starting the WebRTC input worker")?;
    Ok(InputHandle(tx))
}

/// The summary is for the tests; the spawned worker logs it and drops it.
fn run(rx: mpsc::Receiver<Message>, make: Make) -> Option<Summary> {
    let mut early = VecDeque::new();
    let mut control = None;
    let session = loop {
        match rx.recv() {
            Ok(Message::Bind(session)) => break session,
            Ok(Message::Control(sender)) => control = Some(sender),
            Ok(Message::Closed) | Err(_) => return None,
            Ok(message) => early.push_back(message),
        }
    };
    let mut core = Core::new(session, make, Instant::now());
    if let Some(sender) = control {
        core.control = Some(Box::new(move |data| sender.send(data, false)));
    }
    // A panic must not leave a key or a button down on the desktop: whatever happened, release
    // what the client held and unplug its pads, and say so loudly.
    let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve(&mut core, &rx, early)
    }));
    if let Err(panic) = served {
        let what = panic_text(panic.as_ref());
        tracing::error!(panic = %what, "WebRTC input worker panicked; releasing held keys, buttons and controllers");
        core.session.launch.warnings.set(
            "input_worker",
            format!("The input worker failed ({what}); keyboard, mouse and controller input stopped. Reconnect to restore it."),
        );
    }
    let finished =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| core.finish(Instant::now())));
    if finished.is_err() {
        tracing::error!("WebRTC input worker failed again while releasing; dropping the injector");
        core.sink = None;
    }
    Some(core.summary())
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// The worker's loop, until the transport closes, every handle is gone or the session stops.
fn serve(core: &mut Core, rx: &mpsc::Receiver<Message>, mut early: VecDeque<Message>) {
    loop {
        let message = if let Some(message) = early.pop_front() {
            Some(message)
        } else {
            match core.wait(Instant::now()) {
                None => match rx.recv() {
                    Ok(message) => Some(message),
                    Err(_) => break,
                },
                Some(timeout) => match rx.recv_timeout(timeout) {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                },
            }
        };
        let now = Instant::now();
        core.dequeued = Some(now);
        match message {
            Some(Message::Gamepad(text, received)) => core.gamepad(&text, received),
            Some(Message::Input(text, received)) => core.input(&text, received),
            Some(Message::GamepadClosed) => core.unplug(now),
            Some(Message::InputClosed) => core.release_held(now),
            Some(Message::Control(sender)) => {
                core.control = Some(Box::new(move |data| sender.send(data, false)));
            }
            Some(Message::Closed) => break,
            Some(Message::Bind(_)) | None => {}
        }
        if !core.poll(now) {
            break;
        }
    }
}

#[cfg(test)]
pub(crate) use recording::Recorder;

#[cfg(test)]
mod recording {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Log {
        events: Vec<Input>,
        feedback: Vec<(u16, u16, Vec<u8>)>,
        made: usize,
        alive: usize,
        fail_makes: usize,
        denied_kinds: Vec<u16>,
        panic_next: bool,
        apply_delay: Duration,
    }

    /// An injector that records what it is handed and injects nothing: a test fixture's input
    /// device, shared by every sink it makes.
    #[derive(Default, Clone)]
    pub(crate) struct Recorder(Arc<Mutex<Log>>);

    impl Recorder {
        pub(crate) fn sink(&self) -> Box<dyn Sink> {
            let mut log = self.0.lock().unwrap();
            log.made += 1;
            log.alive += 1;
            Box::new(Recording(self.clone()))
        }

        /// Every event injected so far, in order.
        pub(crate) fn events(&self) -> Vec<Input> {
            self.0.lock().unwrap().events.clone()
        }

        pub(crate) fn clear(&self) {
            self.0.lock().unwrap().events.clear();
        }

        /// Sinks made so far, and how many are still alive.
        pub(crate) fn sinks(&self) -> (usize, usize) {
            let log = self.0.lock().unwrap();
            (log.made, log.alive)
        }

        /// Rumble or effects a controller's driver posted: (slot, kind, data).
        pub(crate) fn post_feedback(&self, slot: u16, kind: u16, data: &[u8]) {
            self.0
                .lock()
                .unwrap()
                .feedback
                .push((slot, kind, data.to_vec()));
        }

        /// The next `count` attempts to make a sink fail.
        pub(crate) fn fail_makes(&self, count: usize) {
            self.0.lock().unwrap().fail_makes = count;
        }

        /// The next `apply` panics, as a bug in the injector would.
        pub(crate) fn panic_once(&self) {
            self.0.lock().unwrap().panic_next = true;
        }

        /// Every `apply` takes this long, as `SendInput` does behind a slow low-level hook.
        pub(crate) fn slow_apply(&self, delay: Duration) {
            self.0.lock().unwrap().apply_delay = delay;
        }

        pub(crate) fn forbid_feedback(&self, kind: u16) {
            self.0.lock().unwrap().denied_kinds.push(kind);
        }

        /// Whether a sink could be made.
        pub(crate) fn make(&self) -> Result<Box<dyn Sink>> {
            let mut log = self.0.lock().unwrap();
            if log.fail_makes > 0 {
                log.fail_makes -= 1;
                anyhow::bail!("the test injector is unavailable");
            }
            drop(log);
            Ok(self.sink())
        }
    }

    struct Recording(Recorder);

    impl Sink for Recording {
        fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
            let mut log = (self.0).0.lock().unwrap();
            if std::mem::take(&mut log.panic_next) {
                drop(log);
                panic!("the test injector panicked");
            }
            let delay = log.apply_delay;
            log.events.extend_from_slice(events);
            drop(log);
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            Vec::new()
        }
        fn next_due(&self) -> Option<Instant> {
            None
        }
        fn due(&mut self) -> Result<()> {
            Ok(())
        }
        fn refresh(&mut self) -> Result<()> {
            Ok(())
        }
        fn geometry(&mut self, _: &str, _: u32, _: u32) {}
        fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
            std::mem::take(&mut (self.0).0.lock().unwrap().feedback)
        }
        fn feedback_allowed(&self, kind: u16) -> bool {
            !(self.0).0.lock().unwrap().denied_kinds.contains(&kind)
        }
    }

    impl Drop for Recording {
        fn drop(&mut self) {
            (self.0).0.lock().unwrap().alive -= 1;
        }
    }
}

#[cfg(test)]
mod tests;
