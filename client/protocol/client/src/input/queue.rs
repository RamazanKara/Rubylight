//! The input queue: the `LiSend*` API, with moonlight-common-c's batching (relative motion
//! summed, absolute motion, controller axes and motion sensors sent as their latest state,
//! pen moves collapsed, text split per code point) and Nightjar's network grace rule
//! (input held back while the host is silent, then replaced by the current state).

mod hold;
mod poll;
mod send;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use hold::Held;

use super::packets::{touch_event, InputEvent, MAX_CONTROLLERS};
use crate::control::negotiation::HostCapabilities;
use crate::control::wire::Delivery;

/// Most packets waiting to be sent (`MAX_QUEUED_INPUT_PACKETS`).
pub const MAX_QUEUED: usize = 150;
/// Releases a full queue keeps room for above [`MAX_QUEUED`]: a key or button going up must reach
/// the PC, so a burst of presses cannot take the room its release needs.
const RELEASE_RESERVE: usize = 8;
/// Mouse and pen motion closer together than this waits to be batched with the next.
pub const MOTION_BATCHING_INTERVAL: Duration = Duration::from_millis(1);
/// A relative motion floor that a 1 kHz mouse's reports, jitter included, stay clear of, while
/// a 4 or 8 kHz mouse is merged ([`InputQueue::set_relative_motion_floor`]).
pub const FAST_MOUSE_FLOOR: Duration = Duration::from_micros(500);
/// Text waits this long after earlier input so Windows has processed key events first.
pub const TEXT_SETTLE: Duration = Duration::from_millis(50);
const WHEEL_DELTA: i32 = 120;
const MAX_MOTION_TYPES: usize = 2;
/// The controller slots GFE's GameStream takes: its gamepad numbers fold past the fourth, and
/// its mask has four bits.
const GFE_CONTROLLERS: u8 = 4;

/// An encoded input packet for [`crate::control::ControlSession::send_input`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputPacket {
    pub data: Vec<u8>,
    pub channel: u8,
    pub delivery: Delivery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError {
    /// Input has stopped (`-2` in the C API).
    Stopped,
    /// The host does not take this event (`LI_ERR_UNSUPPORTED`).
    Unsupported,
    /// The queue is full.
    QueueFull,
    /// A parameter is out of range (`-3`).
    Invalid,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stopped => "input has stopped",
            Self::Unsupported => "the host does not support this input",
            Self::QueueFull => "the input queue is full",
            Self::Invalid => "invalid input parameter",
        })
    }
}

impl std::error::Error for InputError {}

#[derive(Clone, Debug, PartialEq)]
struct Pending {
    event: InputEvent,
    channel: u8,
    delivery: Delivery,
}

impl Pending {
    fn new(event: InputEvent) -> Self {
        Self {
            channel: event.channel(),
            delivery: event.default_delivery(),
            event,
        }
    }
}

enum Entry {
    Event(Pending),
    /// A controller state that later calls with the same buttons update in place.
    Controller {
        id: u64,
        state: Pending,
    },
    RelativeMouse,
    AbsoluteMouse,
    Motion {
        number: u8,
        kind: u8,
    },
    Text(Vec<u8>),
}

/// Whether `entry` is something going up: a key or button release, a neutral controller, or a
/// touch, pen or controller contact ending. A full queue keeps [`RELEASE_RESERVE`] for these.
fn is_release(entry: &Entry) -> bool {
    match entry {
        Entry::Event(Pending { event, .. }) => match event {
            InputEvent::Keyboard { down, .. } | InputEvent::MouseButton { down, .. } => !*down,
            InputEvent::Touch { event, .. }
            | InputEvent::Pen { event, .. }
            | InputEvent::ControllerTouch { event, .. }
            | InputEvent::Touchpad { event, .. } => {
                matches!(*event, touch_event::UP | touch_event::CANCEL | touch_event::CANCEL_ALL | touch_event::HOVER_LEAVE)
            }
            _ => false,
        },
        Entry::Controller { state, .. } => matches!(
            &state.event,
            InputEvent::Controller { buttons: 0, left_trigger: 0, right_trigger: 0, left_stick: (0, 0), right_stick: (0, 0), .. }
        ),
        _ => false,
    }
}

#[derive(Clone, Copy, Default)]
struct SensorState {
    xyz: [f32; 3],
    dirty: bool,
}

/// The input stream of one connection. Producers call the `send_*` methods from any thread
/// (behind a lock); the control thread calls [`Self::poll`] and sends what it returns.
pub struct InputQueue {
    sunshine: bool,
    feature_flags: u32,
    batched_scroll: bool,
    stopped: bool,
    unbatched: bool,
    relative_floor: Duration,
    entries: VecDeque<Entry>,
    next_id: u64,
    controller_batch: [Option<u64>; MAX_CONTROLLERS as usize],
    relative: (i32, i32),
    relative_dirty: bool,
    absolute: (i16, i16, i16, i16),
    absolute_dirty: bool,
    sensors: [[SensorState; MAX_MOTION_TYPES]; MAX_CONTROLLERS as usize],
    pen_buttons: u8,
    touchpad_buttons: u8,
    pen_barrel_roll: bool,
    controller_identity: bool,
    scroll_accumulator: i32,
    /// Fractional wheel units (1/120 of a click) not yet sent, per axis: a trackpad reports a
    /// fraction of a notch per event, and each turn carries the fraction it left.
    wheel_units: (f64, f64),
    position: (f32, f32),
    last_mouse: Option<Instant>,
    last_pen: Option<Instant>,
    text_ready_at: Option<Instant>,
    holding: bool,
    held: Held,
    /// Counts relative mouse motion and controller packets as they leave, for Frame Warp.
    frame_warp: Option<std::sync::Arc<crate::frame_warp::FrameWarp>>,
}

impl InputQueue {
    /// Queues GFE's haptics enable packet for hosts that take it (7.1 and later).
    pub fn new(caps: &HostCapabilities) -> Self {
        let mut queue = Self {
            sunshine: caps.is_sunshine(),
            feature_flags: caps.feature_flags,
            batched_scroll: caps.version_at_least(7, 1, 409) && !caps.is_sunshine(),
            stopped: false,
            unbatched: false,
            relative_floor: Duration::ZERO,
            entries: VecDeque::new(),
            next_id: 0,
            controller_batch: [None; MAX_CONTROLLERS as usize],
            relative: (0, 0),
            relative_dirty: false,
            absolute: (0, 0, 0, 0),
            absolute_dirty: false,
            sensors: [[SensorState::default(); MAX_MOTION_TYPES]; MAX_CONTROLLERS as usize],
            pen_buttons: 0,
            touchpad_buttons: 0,
            pen_barrel_roll: false,
            controller_identity: false,
            scroll_accumulator: 0,
            wheel_units: (0.0, 0.0),
            position: (0.5, 0.5),
            last_mouse: None,
            last_pen: None,
            text_ready_at: None,
            holding: false,
            held: Held::default(),
            frame_warp: None,
        };
        if caps.version_at_least(7, 1, 0) {
            let _ = queue.offer(Entry::Event(Pending::new(InputEvent::EnableHaptics)));
        }
        queue
    }

    /// The host's `x-ss-general.featureFlags`, for a client choosing how to send.
    pub fn feature_flags(&self) -> u32 {
        self.feature_flags
    }

    /// Refuses further input; what is queued can still be drained.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Never sends anything: drops what is queued (including the haptics enable) and refuses
    /// input, as when moonlight-common-c never starts the input stream of a remote monitor.
    pub fn disable(&mut self) {
        self.entries.clear();
        self.stopped = true;
    }

    /// Sends mouse and pen motion at once instead of waiting [`MOTION_BATCHING_INTERVAL`]
    /// to batch it (Nightjar's "Unbatched input"). Motion is still merged while it waits in
    /// the queue behind other packets.
    pub fn set_unbatched(&mut self, unbatched: bool) {
        self.unbatched = unbatched;
    }

    /// Relative mouse motion leaves at most once per `floor` after the last packet, unbatched
    /// too, so a fast mouse (4 or 8 kHz) is merged instead of sending a packet per report. The
    /// floor counts from the send, which comes after the report and jitters, and Windows rounds
    /// the control thread's wait up to a whole millisecond: keep it well under the slower
    /// mice's report interval ([`FAST_MOUSE_FLOOR`]).
    pub fn set_relative_motion_floor(&mut self, floor: Duration) {
        self.relative_floor = floor;
    }

    /// Sends a pen's known barrel roll as 0x5500000B (`rubylight_protocol::pen`). Only for a
    /// host with both `PenBarrelRollVersion` and `x-ss-general.penBarrelRoll`; otherwise the
    /// roll is dropped and pens go as the stock 0x55000003.
    pub fn set_pen_barrel_roll(&mut self, offered: bool) {
        self.pen_barrel_roll = offered;
    }

    /// Sends controller identities (0x55000080): only for a host with
    /// `ControllerIdentityVersion` (`Feature::ControllerIdentity`). Otherwise
    /// identities are dropped. The C button's flag (0x40_0000) is the caller's
    /// to gate: it is also a Steam Controller's left grip touch, so
    /// [`Self::send_controller`] never clears it (an app that sends C drops it
    /// from a host without identities itself, as the desktop does).
    pub fn set_controller_identity(&mut self, offered: bool) {
        self.controller_identity = offered;
    }

    /// Whether [`Self::send_controller_identity`] reaches the host. An app that
    /// sends the C button gates that too (the desktop does); the flag is also a
    /// Steam Controller's left grip, which [`Self::send_controller`] keeps.
    pub fn controller_identity(&self) -> bool {
        self.controller_identity
    }

    /// The controller slots this host takes: four on GFE's GameStream, whose gamepad numbers
    /// [`Self::send_controller`] folds past the fourth, and 16 on Sunshine and its forks. An app
    /// announces and sends no more than these, so it can tell the player why a pad does nothing.
    pub fn max_controllers(&self) -> u8 {
        if self.sunshine { MAX_CONTROLLERS } else { GFE_CONTROLLERS }
    }

    /// Counts what leaves from now on for Frame Warp (`crate::frame_warp`).
    pub fn set_frame_warp(&mut self, frame_warp: Option<std::sync::Arc<crate::frame_warp::FrameWarp>>) {
        self.frame_warp = frame_warp;
    }

    fn motion_interval(&self) -> Duration {
        if self.unbatched {
            Duration::ZERO
        } else {
            MOTION_BATCHING_INTERVAL
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && !self.holding
    }

    /// Whether input is held back for a silent host.
    pub fn is_holding(&self) -> bool {
        self.holding
    }

    fn check(&self) -> Result<(), InputError> {
        if self.stopped {
            Err(InputError::Stopped)
        } else {
            Ok(())
        }
    }

    fn offer(&mut self, entry: Entry) -> Result<(), InputError> {
        if self.entries.len() >= MAX_QUEUED {
            let made_room = self.drop_unreliable();
            if !made_room && (!is_release(&entry) || self.entries.len() >= MAX_QUEUED + RELEASE_RESERVE) {
                return Err(InputError::QueueFull);
            }
        }
        self.entries.push_back(entry);
        Ok(())
    }

    /// Drops the oldest event the protocol allows to be lost, to make room for one it does not
    /// (a release, a key, a click): a full queue must never cost the host a button it still holds
    /// down. Moves and hovers are the only entries that go.
    fn drop_unreliable(&mut self) -> bool {
        let Some(at) = self.entries.iter().position(|entry| {
            matches!(entry, Entry::Event(pending) if pending.delivery == Delivery::Unreliable)
        }) else {
            return false;
        };
        self.entries.remove(at);
        true
    }

    fn offer_event(&mut self, event: InputEvent) -> Result<(), InputError> {
        self.offer(Entry::Event(Pending::new(event)))
    }
}

#[cfg(test)]
mod tests;
