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
//!
//! Guests of a console stream (docs/browser-guests.md) send through the owner's worker: their
//! pads take slots in the owner's table, under ids of their own, so the injector's one active
//! mask covers everyone's and the slot limit holds for all of them. Each guest's input is checked
//! against its own permission here, on top of the owner device's: a view-only guest's input never
//! reaches the injector, whatever its page sends.

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
        gamepad::{self, Action, Control, GamepadMessage, Gamepads, MAX_GAMEPADS, Outcome},
        guests::Permission,
        input::InputChannel,
    },
};
use butterpollo_windows::input::{Injector, PadReport};
use std::{
    collections::{BTreeMap, HashMap, VecDeque, hash_map::Entry},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How often the injector's display lookup and pointer refresh run while input flows (the
/// GameStream control loop runs them every pass of its 8 ms poll).
const REFRESH: Duration = Duration::from_millis(8);
/// An injector call slower than this is counted, and logged with the kinds of event in it.
const SLOW_APPLY: Duration = Duration::from_millis(2);
/// How many slow calls a session logs; the summary's `slow_calls` counts them all.
const SLOW_LOGGED: u32 = 16;
/// How often the injector is polled for rumble while a controller is plugged in, as the
/// GameStream control loop does.
const FEEDBACK_POLL: Duration = Duration::from_millis(8);
/// How often a worker that holds something looks at whether the session is stopping, in case the
/// transport takes a moment to report it closed.
const STOP_POLL: Duration = Duration::from_millis(100);
/// A failed injector is made again no sooner than this.
const RETRY: Duration = Duration::from_millis(500);
/// A guest's controllers are numbered above every id an owner's browser uses (the Gamepad API
/// counts from 0): this bit, the guest's tag, then the id its own browser gave.
const GUEST_IDS: u32 = 1 << 31;
const GUEST_ID_BITS: u32 = 16;
const MAX_GUEST_TAG: u16 = 0x7fff;
/// Pads one guest may plug in, and the slots the guests together leave to the owner's.
const GUEST_PADS: usize = 4;
const OWNER_PADS: usize = 4;
/// A guest's gamepad and input messages waiting for the worker before they give way: past this,
/// motion is dropped and a snapshot replaces the guest's last one still waiting ([`Yields`]).
const GUEST_QUEUE: usize = 64;
/// A guest's messages waiting in all, so a page that sends faster than the desktop takes them
/// cannot fill the host's memory. Past this even keys, buttons and pads coming and going are
/// dropped, and the worker lets go of everything the guest holds when it gets there.
const GUEST_BACKLOG: usize = 256;

/// The id in the shared table of a guest's controller; None for an id out of range.
fn guest_controller(tag: u16, controller_id: u32) -> Option<u32> {
    (controller_id < 1 << GUEST_ID_BITS)
        .then_some(GUEST_IDS | u32::from(tag) << GUEST_ID_BITS | controller_id)
}

/// The guest tag and the guest's own id of a controller in the shared table; None for the
/// owner's.
fn guest_of(controller_id: u32) -> Option<(u16, u32)> {
    (controller_id & GUEST_IDS != 0).then_some((
        ((controller_id & !GUEST_IDS) >> GUEST_ID_BITS) as u16,
        controller_id & ((1 << GUEST_ID_BITS) - 1),
    ))
}

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
    /// The guests' queue has messages ([`GuestInbox`]).
    Guests,
}

/// What a guest's transport tells the worker, by guest id.
enum GuestMessage {
    /// A guest of the stream joined, with what it may send.
    Joined(u64, Permission),
    Gamepad(u64, String, Instant),
    Input(u64, String, Instant),
    Control(u64, ControlSender),
    GamepadClosed(u64),
    InputClosed(u64),
    /// A guest left: its pads are unplugged and what it held is released.
    Left(u64),
    /// Some of a guest's keys, buttons or pads' comings and goings were dropped
    /// ([`GUEST_BACKLOG`]): what it holds is released and its pads are unplugged, as if it had
    /// let go of everything.
    Lost(u64),
}

impl GuestMessage {
    fn guest(&self) -> u64 {
        match *self {
            Self::Joined(id, _)
            | Self::Gamepad(id, ..)
            | Self::Input(id, ..)
            | Self::Control(id, _)
            | Self::GamepadClosed(id)
            | Self::InputClosed(id)
            | Self::Left(id)
            | Self::Lost(id) => id,
        }
    }

    /// What the page sends, as opposed to the guest's comings and goings, which always queue.
    fn data(&self) -> bool {
        matches!(self, Self::Gamepad(..) | Self::Input(..))
    }
}

/// How a guest's message gives way once its guest has [`GUEST_QUEUE`] waiting.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Yields {
    /// Motion, scrolling and what the worker could not read anyway: dropped, which leaves
    /// nothing held.
    Drop,
    /// A whole state (a pad's buttons and sticks, the pointer's position): replaces the guest's
    /// last one of the same kind still waiting, unless something that must keep its order came
    /// after it.
    Replace(Snapshot),
    /// Keys, buttons, text, pads coming and going, the guest's comings and goings: queued until
    /// [`GUEST_BACKLOG`].
    Keep,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Snapshot {
    Pad(u32),
    Pointer,
}

impl Yields {
    /// What a page's `gamepad` or `input` message is, from its `type` and `controllerId` alone.
    fn of(message: &GuestMessage) -> Self {
        #[derive(serde::Deserialize)]
        struct Peek<'a> {
            #[serde(rename = "type", borrow)]
            kind: std::borrow::Cow<'a, str>,
            #[serde(rename = "controllerId")]
            controller_id: Option<u32>,
        }
        let (text, gamepad) = match message {
            GuestMessage::Gamepad(_, text, _) => (text, true),
            GuestMessage::Input(_, text, _) => (text, false),
            _ => return Self::Keep,
        };
        let Ok(peek) = serde_json::from_str::<Peek>(text) else {
            return Self::Drop;
        };
        match (gamepad, &*peek.kind, peek.controller_id) {
            (true, "gamepad-state", Some(id)) => Self::Replace(Snapshot::Pad(id)),
            (true, "gamepad-connected" | "gamepad-disconnected", _) => Self::Keep,
            (false, "mouse-position", _) => Self::Replace(Snapshot::Pointer),
            (false, "key" | "mouse-button" | "text", _) => Self::Keep,
            _ => Self::Drop,
        }
    }
}

/// The guests' messages, beside the owner's queue: the worker takes one only while the owner's
/// queue is empty, so no guest delays the owner's input. A guest's messages give way past
/// [`GUEST_QUEUE`] ([`Yields`]) and stop at [`GUEST_BACKLOG`].
#[derive(Default)]
struct GuestInbox {
    state: Mutex<Inbox>,
    /// A [`Message::Guests`] is in the owner's queue and the worker has not taken it yet.
    woken: AtomicBool,
}

#[derive(Default)]
struct Inbox {
    queue: VecDeque<(GuestMessage, Yields)>,
    /// Per guest with data waiting or a [`GuestMessage::Lost`] last in the queue; no entry for
    /// none.
    waiting: HashMap<u64, Backlog>,
}

#[derive(Default, Debug)]
struct Backlog {
    /// Data messages queued.
    data: usize,
    /// The guest's last message queued is a [`GuestMessage::Lost`], which covers what is
    /// dropped until another queues behind it.
    lost: bool,
}

impl Inbox {
    /// The guest's last snapshot of `kind` still waiting, with nothing that keeps its order
    /// queued for the guest after it.
    fn snapshot(&mut self, guest: u64, kind: Snapshot) -> Option<&mut GuestMessage> {
        for (message, yields) in self.queue.iter_mut().rev() {
            if message.guest() != guest {
                continue;
            }
            match *yields {
                Yields::Replace(k) if k == kind => return Some(message),
                Yields::Keep => return None,
                _ => {}
            }
        }
        None
    }

    /// Queue `message`, or say why not: false when it was dropped. `wake`: the worker needs
    /// waking for what was queued.
    fn push(&mut self, message: GuestMessage) -> (bool, bool) {
        let guest = message.guest();
        let yields = Yields::of(&message);
        let data = self.waiting.get(&guest).map_or(0, |b| b.data);
        if message.data() && data >= GUEST_QUEUE {
            match yields {
                Yields::Drop => return (false, false),
                // The worker takes the guest's waiting messages before it sleeps: no wake.
                Yields::Replace(kind) => {
                    if let Some(older) = self.snapshot(guest, kind) {
                        *older = message;
                        return (true, false);
                    }
                }
                Yields::Keep => {}
            }
            if data >= GUEST_BACKLOG {
                let backlog = self.waiting.entry(guest).or_default();
                if std::mem::replace(&mut backlog.lost, true) {
                    return (false, false);
                }
                self.queue
                    .push_back((GuestMessage::Lost(guest), Yields::Keep));
                return (false, true);
            }
        }
        let backlog = self.waiting.entry(guest).or_default();
        backlog.lost = false;
        if message.data() {
            backlog.data += 1;
        } else if backlog.data == 0 {
            self.waiting.remove(&guest);
        }
        self.queue.push_back((message, yields));
        (true, true)
    }

    fn pop(&mut self) -> Option<GuestMessage> {
        let (message, _) = self.queue.pop_front()?;
        let guest = message.guest();
        if let Entry::Occupied(mut backlog) = self.waiting.entry(guest) {
            match message {
                GuestMessage::Lost(_) => backlog.get_mut().lost = false,
                _ if message.data() => backlog.get_mut().data -= 1,
                _ => {}
            }
            if backlog.get().data == 0 && !backlog.get().lost {
                backlog.remove();
            }
        }
        Some(message)
    }
}

impl GuestInbox {
    /// Queue `message` and wake the worker. False when it was dropped ([`Yields`]).
    fn push(&self, tx: &mpsc::Sender<Message>, message: GuestMessage) -> bool {
        let (queued, wake) = self.state.lock().unwrap().push(message);
        if wake && !self.woken.swap(true, Ordering::AcqRel) {
            let _ = tx.send(Message::Guests);
        }
        queued
    }

    fn pop(&self) -> Option<GuestMessage> {
        self.state.lock().unwrap().pop()
    }
}

/// The worker's end of an [`InputHandle`].
pub(crate) struct Queues {
    rx: mpsc::Receiver<Message>,
    guests: Arc<GuestInbox>,
}

/// A worker's handle and the queues it reads.
pub(crate) fn channel() -> (InputHandle, Queues) {
    let (tx, rx) = mpsc::channel();
    let guests = Arc::new(GuestInbox::default());
    (InputHandle(tx, guests.clone()), Queues { rx, guests })
}

/// The transport's end of a session's input worker. Every call only queues; none blocks.
#[derive(Clone)]
pub struct InputHandle(mpsc::Sender<Message>, Arc<GuestInbox>);

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
        (Self(tx, Arc::default()), drain)
    }

    /// A text frame from the `gamepad` channel, received at `now`. A binary or non-UTF-8 frame is
    /// not part of the protocol and is dropped with a warning.
    pub fn gamepad(&self, data: Vec<u8>, binary: bool, now: Instant) {
        if let Some(text) = text(data, binary, "gamepad", None) {
            let _ = self.0.send(Message::Gamepad(text, now));
        }
    }

    /// A text frame from the `input` channel.
    pub fn input(&self, data: Vec<u8>, binary: bool) {
        if let Some(text) = text(data, binary, "input", None) {
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

    /// A guest of this session's stream, sending through this worker with `permission`. None
    /// for a view-only guest: nothing it sends reaches the worker.
    pub fn guest(&self, id: u64, permission: Permission) -> Option<GuestHandle> {
        if !permission.gamepads() {
            return None;
        }
        let guest = GuestHandle {
            tx: self.0.clone(),
            inbox: self.1.clone(),
            id,
            permission,
            warned: AtomicBool::new(false),
            flooded: AtomicBool::new(false),
        };
        guest.push(GuestMessage::Joined(id, permission));
        Some(guest)
    }
}

/// A guest's end of the owner's input worker, for the guest transport's hooks. Every call only
/// queues, and only what `permission` allows. Dropping it is the guest leaving: its pads are
/// unplugged and its keys let go.
pub struct GuestHandle {
    tx: mpsc::Sender<Message>,
    inbox: Arc<GuestInbox>,
    id: u64,
    permission: Permission,
    /// A frame that is not text was logged once; a page sending more must not flood the log.
    warned: AtomicBool,
    /// Dropping messages over [`GUEST_QUEUE`] was logged once.
    flooded: AtomicBool,
}

impl GuestHandle {
    fn push(&self, message: GuestMessage) {
        if !self.inbox.push(&self.tx, message) && !self.flooded.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                guest = self.id,
                "WebRTC input: a guest sends faster than the desktop takes it; dropping the excess"
            );
        }
    }

    pub fn gamepad(&self, data: Vec<u8>, binary: bool, now: Instant) {
        if let Some(text) = text(data, binary, "gamepad", Some(&self.warned)) {
            self.push(GuestMessage::Gamepad(self.id, text, now));
        }
    }

    pub fn input(&self, data: Vec<u8>, binary: bool) {
        if !self.permission.keyboard_mouse() {
            return;
        }
        if let Some(text) = text(data, binary, "input", Some(&self.warned)) {
            self.push(GuestMessage::Input(self.id, text, Instant::now()));
        }
    }

    pub fn channel_closed(&self, label: &str) {
        let message = match label {
            "input" => GuestMessage::InputClosed(self.id),
            "gamepad" => GuestMessage::GamepadClosed(self.id),
            _ => return,
        };
        self.push(message);
    }

    /// The guest's `control` channel, for its pads' rumble and mouse mode.
    pub fn control(&self, sender: ControlSender) {
        self.push(GuestMessage::Control(self.id, sender));
    }
}

impl Drop for GuestHandle {
    fn drop(&mut self) {
        self.push(GuestMessage::Left(self.id));
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

/// The frame's text. A guest's frames that are not text are logged loudly once (`warned`), then
/// quietly.
fn text(data: Vec<u8>, binary: bool, channel: &str, warned: Option<&AtomicBool>) -> Option<String> {
    let why = if binary {
        "WebRTC input: a binary frame is not part of the protocol; dropped"
    } else {
        match String::from_utf8(data) {
            Ok(text) => return Some(text),
            Err(_) => "WebRTC input: a frame that is not UTF-8; dropped",
        }
    };
    if warned.is_some_and(|warned| warned.swap(true, Ordering::Relaxed)) {
        tracing::debug!(channel, "{why}");
    } else {
        tracing::warn!(channel, "{why}");
    }
    None
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

/// A co-op player's input, which drives its slot's pad and never the desktop.
struct CoopSink {
    h: Shared,
    player: crate::nucleus::Player,
    warnings: Arc<butterpollo_core::session::Warnings>,
}

impl CoopSink {
    fn create(h: &Shared, s: &Session) -> Result<Box<dyn Sink>> {
        let c = effective_config(h, &s.launch)?;
        let player =
            crate::nucleus::Player::of(h, &s.launch, &c).context("the co-op session ended")?;
        Ok(Box::new(Self {
            h: h.clone(),
            player,
            warnings: s.launch.warnings.clone(),
        }))
    }
}

impl Sink for CoopSink {
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        for event in events {
            self.player.input(event.clone(), &self.warnings);
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
    fn geometry(&mut self, _output: &str, _width: u32, _height: u32) {}
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        self.player
            .feedback(&self.h)
            .into_iter()
            .filter_map(|report| match report {
                PadReport::Feedback { id, kind, data } => Some((id, kind, data)),
                _ => None,
            })
            .collect()
    }
    fn feedback_allowed(&self, kind: u16) -> bool {
        self.player.feedback_allowed(kind)
    }
}

type Make = Box<dyn FnMut(&Session) -> Result<Box<dyn Sink>> + Send>;
type Reply = Box<dyn FnMut(Vec<u8>) -> bool + Send>;

/// The kinds of event in a call, each once, in the order they first appear.
fn event_kinds(events: &[Input]) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for event in events {
        let kind = format!("{event:?}")
            .split([' ', '(', '{'])
            .next()
            .unwrap_or("")
            .to_string();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

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
    /// `SendInput`, is below: it can take milliseconds (on the 780M laptop the first left press
    /// after idle takes 20 to 38 ms, with or without a host), as it does for ENet, and that is not
    /// the host's queue.
    pub queued: u64,
    pub queue_p50_us: u64,
    pub queue_max_us: u64,
    pub queue_p99_us: u64,
    /// The parts of that delay: waiting in the channel for the worker, and the injector's
    /// periodic refresh that runs before the events; then the injector call itself.
    pub wait_p50_us: u64,
    pub wait_max_us: u64,
    pub wait_p99_us: u64,
    pub refresh_max_us: u64,
    pub refresh_p99_us: u64,
    pub apply_max_us: u64,
    pub apply_p99_us: u64,
    /// Injector calls that took [`SLOW_APPLY`] or more. Only the first 16 are logged; this is all.
    pub slow_calls: u64,
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

    /// The upper edge of the bucket the 99th percentile falls in, never above the maximum. It is
    /// a figure over messages: one that refreshed the pointer counts once, like any other.
    fn p99_us(&self) -> u64 {
        self.percentile_us(99)
    }

    fn median_us(&self) -> u64 {
        self.percentile_us(50)
    }

    fn percentile_us(&self, percent: u64) -> u64 {
        let rank = (self.count * percent).div_ceil(100);
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

/// A guest sending through the owner's worker.
struct Guest {
    /// The tag in its controllers' ids ([`guest_controller`]).
    tag: u16,
    permission: Permission,
    /// Its own keys and buttons, released when it leaves.
    channel: InputChannel,
    control: Option<Reply>,
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
    /// The stream's guests by id, whose pads share `pads`.
    guests: BTreeMap<u64, Guest>,
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
            guests: BTreeMap::new(),
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
            queue_p50_us: self.delays.median_us(),
            queue_max_us: self.delays.max_us,
            queue_p99_us: self.delays.p99_us(),
            wait_p50_us: self.waits.median_us(),
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
        let (Control::MouseMode { controller_id, .. } | Control::Rumble { controller_id, .. }) =
            control;
        if let Some((tag, own)) = guest_of(*controller_id) {
            // A guest's pad answers on the guest's channel, under the id its browser knows; a
            // guest that left gets nothing, and nothing goes to the owner instead.
            let Some(send) = self
                .guests
                .values_mut()
                .find(|g| g.tag == tag)
                .and_then(|g| g.control.as_mut())
            else {
                return;
            };
            let mut control = control.clone();
            let (Control::MouseMode { controller_id, .. } | Control::Rumble { controller_id, .. }) =
                &mut control;
            *controller_id = own;
            if matches!(control, Control::Rumble { .. }) {
                self.summary.rumble_sent += 1;
            }
            send(control.to_json().into_bytes());
            return;
        }
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
        if done.saturating_duration_since(started) >= SLOW_APPLY {
            self.summary.slow_calls += 1;
            if self.slow_logged < SLOW_LOGGED {
                self.slow_logged += 1;
                tracing::info!(
                    took_us = done.saturating_duration_since(started).as_micros() as u64,
                    kinds = ?event_kinds(&events),
                    "WebRTC input: a slow injection"
                );
            }
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
        let busy = self.sink.is_some()
            || self.pads.active_mask() != 0
            || !self.channel.held().is_empty()
            || self.guests.values().any(|g| !g.channel.held().is_empty());
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
    /// The guests' stay.
    fn unplug(&mut self, now: Instant) {
        let ids = self.pads.controller_ids();
        if !ids.iter().any(|&id| guest_of(id).is_some()) {
            let actions = self.pads.close();
            self.actions(actions, now);
            return;
        }
        self.disconnect(ids.into_iter().filter(|&id| guest_of(id).is_none()), now);
    }

    fn disconnect(&mut self, ids: impl IntoIterator<Item = u32>, now: Instant) {
        let mut actions = Vec::new();
        for id in ids {
            actions.extend(self.pads.disconnect(id).actions);
        }
        self.actions(actions, now);
    }

    fn guest_joined(&mut self, id: u64, permission: Permission) {
        if self.guests.contains_key(&id) {
            return;
        }
        let Some(tag) = (1..=MAX_GUEST_TAG).find(|t| self.guests.values().all(|g| g.tag != *t))
        else {
            tracing::warn!(guest = id, "WebRTC input: no tag left for a guest");
            return;
        };
        let mode = self.session.stream_mode();
        self.guests.insert(
            id,
            Guest {
                tag,
                permission,
                channel: InputChannel::new(mode.width, mode.height),
                control: None,
            },
        );
    }

    /// One `gamepad` message of a guest, received at `now`. A view-only guest's are dropped
    /// here; a controller-only guest's pads never turn into a mouse.
    fn guest_gamepad(&mut self, id: u64, text: &str, now: Instant) {
        self.summary.gamepad_messages += 1;
        let Some(&Guest {
            tag, permission, ..
        }) = self.guests.get(&id)
        else {
            self.summary.dropped += 1;
            return;
        };
        if !permission.gamepads() || !self.allowed(&controller_event()) {
            self.summary.dropped += 1;
            return;
        }
        let message = match gamepad::parse(text) {
            Ok(message) => message,
            Err(error) => return self.invalid("gamepad", &error),
        };
        let remap = |controller_id| guest_controller(tag, controller_id);
        let outcome = match message {
            GamepadMessage::Connected(mut connected) => {
                remap(connected.controller_id).map(|controller_id| {
                    connected.controller_id = controller_id;
                    if self.pads.slot(controller_id).is_none() && !self.guest_pad_free(tag) {
                        tracing::debug!(guest = id, "WebRTC input: a guest's pad over its share");
                        return Outcome {
                            accepted: false,
                            actions: Vec::new(),
                        };
                    }
                    self.pads
                        .connect_with(&connected, permission.keyboard_mouse())
                })
            }
            GamepadMessage::Disconnected { controller_id } => {
                remap(controller_id).map(|controller_id| self.pads.disconnect(controller_id))
            }
            GamepadMessage::State(mut state) => remap(state.controller_id).map(|controller_id| {
                state.controller_id = controller_id;
                self.pads.state(&state, now)
            }),
        };
        let Some(outcome) = outcome else {
            return self.invalid("gamepad", &"a guest's controller id is out of range");
        };
        if !outcome.accepted && outcome.actions.is_empty() {
            self.summary.gamepad_ignored += 1;
        }
        self.received = Some(now);
        self.actions(outcome.actions, now);
        self.received = None;
    }

    /// Whether the guest tagged `tag` may plug in one more pad: [`GUEST_PADS`] each, and the
    /// guests together leave [`OWNER_PADS`] of the slots to the owner.
    fn guest_pad_free(&self, tag: u16) -> bool {
        let guests: Vec<u16> = self
            .pads
            .controller_ids()
            .into_iter()
            .filter_map(|c| guest_of(c).map(|(t, _)| t))
            .collect();
        guests.iter().filter(|&&t| t == tag).count() < GUEST_PADS
            && guests.len() < MAX_GAMEPADS - OWNER_PADS
    }

    /// A guest's message, in the order its transport sent them.
    fn guest(&mut self, message: GuestMessage, now: Instant) {
        match message {
            GuestMessage::Joined(id, permission) => self.guest_joined(id, permission),
            GuestMessage::Gamepad(id, text, received) => self.guest_gamepad(id, &text, received),
            GuestMessage::Input(id, text, received) => self.guest_input(id, &text, received),
            GuestMessage::Control(id, sender) => self.guest_control(id, sender),
            GuestMessage::GamepadClosed(id) => self.guest_unplug(id, now),
            GuestMessage::InputClosed(id) => self.guest_release(id, now),
            GuestMessage::Left(id) => self.guest_left(id, now),
            GuestMessage::Lost(id) => self.guest_lost(id, now),
        }
    }

    /// One `input` message of a guest: only with the keyboard and mouse permission, and only
    /// the kinds of event that covers.
    fn guest_input(&mut self, id: u64, text: &str, now: Instant) {
        self.summary.input_messages += 1;
        let Some(guest) = self.guests.get_mut(&id) else {
            self.summary.dropped += 1;
            return;
        };
        if !guest.permission.keyboard_mouse() {
            self.summary.dropped += 1;
            return;
        }
        let mask = guest.permission.input_mask();
        let events = match butterpollo_core::webrtc::input::parse(text) {
            Ok(message) => guest.channel.translate(message),
            Err(error) => return self.invalid("input", &error),
        };
        let mut allowed = Vec::with_capacity(events.len());
        for event in events {
            if event.required_permission() & mask == 0 || !self.allowed(&event) {
                self.summary.dropped += 1;
                continue;
            }
            if let Some(guest) = self.guests.get_mut(&id) {
                guest.channel.observe(&event);
            }
            allowed.push(event);
        }
        self.received = Some(now);
        self.inject(allowed, now);
        self.received = None;
    }

    fn guest_control(&mut self, id: u64, sender: ControlSender) {
        if let Some(guest) = self.guests.get_mut(&id) {
            guest.control = Some(Box::new(move |data| sender.send(data, false)));
        }
    }

    /// A guest's `gamepad` channel closed, or it left: unplug its controllers.
    fn guest_unplug(&mut self, id: u64, now: Instant) {
        let Some(tag) = self.guests.get(&id).map(|g| g.tag) else {
            return;
        };
        let ids = self.pads.controller_ids();
        self.disconnect(
            ids.into_iter()
                .filter(|&c| guest_of(c).is_some_and(|(t, _)| t == tag)),
            now,
        );
    }

    /// A guest's `input` channel closed, or it left: let go of what it holds.
    fn guest_release(&mut self, id: u64, now: Instant) {
        let Some(guest) = self.guests.get_mut(&id) else {
            return;
        };
        let events = guest.channel.release_all();
        if !events.is_empty() {
            tracing::info!(
                guest = id,
                count = events.len(),
                "WebRTC input: releasing what a guest held"
            );
            self.inject(events, now);
        }
    }

    fn guest_left(&mut self, id: u64, now: Instant) {
        self.guest_unplug(id, now);
        self.guest_release(id, now);
        self.guests.remove(&id);
    }

    /// Some of a guest's keys, buttons or pads' comings and goings were dropped: rather than
    /// guess what it still holds, let go of all of it and unplug its pads (a pad comes back when
    /// the guest plugs it in again or reloads the page). Only a page that floods the host, or a
    /// desktop that stalls for seconds, gets here.
    fn guest_lost(&mut self, id: u64, now: Instant) {
        tracing::debug!(
            guest = id,
            "WebRTC input: a guest's input was dropped; releasing what it holds"
        );
        self.guest_unplug(id, now);
        self.guest_release(id, now);
    }

    /// Release everything and report. Safe to call twice.
    fn finish(&mut self, now: Instant) {
        let actions = self.pads.close();
        self.actions(actions, now);
        let guests: Vec<u64> = self.guests.keys().copied().collect();
        for id in guests {
            self.guest_release(id, now);
        }
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
            queue_p50_us = summary.queue_p50_us,
            queue_max_us = summary.queue_max_us,
            queue_p99_us = summary.queue_p99_us,
            wait_p50_us = summary.wait_p50_us,
            wait_max_us = summary.wait_max_us,
            wait_p99_us = summary.wait_p99_us,
            refresh_max_us = summary.refresh_max_us,
            refresh_p99_us = summary.refresh_p99_us,
            apply_max_us = summary.apply_max_us,
            apply_p99_us = summary.apply_p99_us,
            slow_calls = summary.slow_calls,
            "WebRTC input summary"
        );
    }
}

/// Start the input worker of a session. It idles until [`InputHandle::bind`] gives it the
/// session; anything the client sends before that waits in its queue.
pub fn spawn(h: Shared) -> Result<InputHandle> {
    let (handle, queues) = channel();
    let make: Make = Box::new(move |session| {
        if session.launch.nucleus.is_some() {
            return CoopSink::create(&h, session);
        }
        #[cfg(test)]
        if let Some(fixture) = &h.reconnect_fixture {
            return fixture.webrtc_input.make();
        }
        NativeSink::create(&h, session)
    });
    std::thread::Builder::new()
        .name("webrtc-input".into())
        .spawn(move || {
            // The control thread's priority: it injects every input. Test
            // workers must not boost their priority or register a Windows task.
            #[cfg(not(test))]
            let _priority = butterpollo_windows::capture::Priority::input();
            run(queues, make)
        })
        .context("starting the WebRTC input worker")?;
    Ok(handle)
}

/// The summary is for the tests; the spawned worker logs it and drops it.
fn run(queues: Queues, make: Make) -> Option<Summary> {
    let mut early = VecDeque::new();
    let mut control = None;
    let session = loop {
        match queues.rx.recv() {
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
        serve(&mut core, &queues, early)
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
fn serve(core: &mut Core, queues: &Queues, mut early: VecDeque<Message>) {
    let rx = &queues.rx;
    loop {
        // The owner's messages first; a guest's is taken only while the owner's queue is empty.
        let mut guest = None;
        let message = if let Some(message) = early.pop_front() {
            Some(message)
        } else {
            match rx.try_recv() {
                Ok(message) => Some(message),
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    guest = queues.guests.pop();
                    if guest.is_some() {
                        None
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
                    }
                }
            }
        };
        let now = Instant::now();
        core.dequeued = Some(now);
        if let Some(guest) = guest {
            core.guest(guest, now);
        }
        match message {
            Some(Message::Gamepad(text, received)) => core.gamepad(&text, received),
            Some(Message::Input(text, received)) => core.input(&text, received),
            Some(Message::GamepadClosed) => core.unplug(now),
            Some(Message::InputClosed) => core.release_held(now),
            Some(Message::Control(sender)) => {
                core.control = Some(Box::new(move |data| sender.send(data, false)));
            }
            Some(Message::Closed) => break,
            // Cleared before the queue is read: a guest's message queued after this wakes the
            // worker again.
            Some(Message::Guests) => queues.guests.woken.store(false, Ordering::Release),
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
