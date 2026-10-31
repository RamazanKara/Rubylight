//! Gamepads from a WebRTC client: the `gamepad-connected`,
//! `gamepad-disconnected` and `gamepad-state` messages mapped to the
//! [`Input`] events the Moonlight path produces, rumble back to the client,
//! and gamepad mouse mode.
//!
//! Everything here is a pure state machine. The caller passes the time of each
//! event and polls [`Gamepads::tick`], so tests need no clock and the host
//! owns the timer.
//!
//! Button order, field names and the mouse-mode constants follow the pinned
//! reference host (Sunshine-Web-RTC `src/webrtc/input_bridge.{h,cpp}`) and the
//! TV app (`tizen/gamepad-input.js`), which is the contract. Where this
//! differs from the reference it says so.
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};

use super::input::{MessageError, check_envelope};
use crate::input::Input;

/// `v` values accepted on gamepad messages. The TV sends 2 and also accepts 1.
const ACCEPTED_VERSIONS: [i64; 2] = [1, 2];
/// `v` of the control messages the host sends: the reference's
/// `GAMEPAD_PROTOCOL_VERSION`. The TV app accepts 1 and 2.
pub const CONTROL_VERSION: u32 = 2;
/// Controller slots in Moonlight's active gamepad mask.
pub const MAX_GAMEPADS: usize = 16;
/// The client re-sends an unchanged state at least this often. Documentation
/// only: the host keeps no timer on it.
pub const KEEPALIVE: Duration = Duration::from_millis(250);
/// Hold Start this long, then release it, to toggle mouse mode (the
/// reference's `MOUSE_EMULATION_LONG_PRESS_TIME`).
pub const MOUSE_MODE_HOLD: Duration = Duration::from_millis(750);
/// Pointer motion period in mouse mode (`MOUSE_EMULATION_POLLING_INTERVAL`).
pub const MOUSE_TICK: Duration = Duration::from_millis(50);
/// Stick-to-pointer speed factor (`MOUSE_EMULATION_MOTION_MULTIPLIER`).
const MOUSE_MOTION_MULTIPLIER: f64 = 4.0;
/// Pointer deadzone in pixels per tick (`MOUSE_EMULATION_DEADZONE`).
const MOUSE_DEADZONE: f64 = 2.0;
/// One wheel notch in Moonlight scroll units.
const WHEEL_NOTCH: i16 = 120;
/// Moonlight's `LI_CTYPE_XBOX`. Every WebRTC gamepad is announced as one.
const CTYPE_XBOX: u8 = 1;
/// Moonlight's `LI_CCAP_ANALOG_TRIGGERS`, `LI_CCAP_RUMBLE` and
/// `LI_CCAP_TRIGGER_RUMBLE`.
const CCAP_ANALOG_TRIGGERS: u16 = 0x01;
const CCAP_RUMBLE: u16 = 0x02;
const CCAP_TRIGGER_RUMBLE: u16 = 0x04;

/// Bits of the `buttons` mask in `gamepad-state`: the Gamepad API's standard
/// buttons in the TV app's order.
pub mod button {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LEFT_BUMPER: u32 = 1 << 4;
    pub const RIGHT_BUMPER: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const LEFT_STICK: u32 = 1 << 8;
    pub const RIGHT_STICK: u32 = 1 << 9;
    pub const DPAD_UP: u32 = 1 << 10;
    pub const DPAD_DOWN: u32 = 1 << 11;
    pub const DPAD_LEFT: u32 = 1 << 12;
    pub const DPAD_RIGHT: u32 = 1 << 13;
    pub const GUIDE: u32 = 1 << 14;
    /// Every bit a client may set.
    pub const ALL: u32 = (1 << 15) - 1;
}

/// Moonlight's controller button flags (the low and high words of
/// `Input::Controller::buttons`).
mod moonlight {
    pub const UP: u32 = 0x0001;
    pub const DOWN: u32 = 0x0002;
    pub const LEFT: u32 = 0x0004;
    pub const RIGHT: u32 = 0x0008;
    pub const PLAY: u32 = 0x0010;
    pub const BACK: u32 = 0x0020;
    pub const LEFT_STICK: u32 = 0x0040;
    pub const RIGHT_STICK: u32 = 0x0080;
    pub const LEFT_BUMPER: u32 = 0x0100;
    pub const RIGHT_BUMPER: u32 = 0x0200;
    pub const SPECIAL: u32 = 0x0400;
    pub const A: u32 = 0x1000;
    pub const B: u32 = 0x2000;
    pub const X: u32 = 0x4000;
    pub const Y: u32 = 0x8000;
}

/// Protocol button bit and Moonlight flag, for every button.
const BUTTON_MAP: [(u32, u32); 15] = [
    (button::A, moonlight::A),
    (button::B, moonlight::B),
    (button::X, moonlight::X),
    (button::Y, moonlight::Y),
    (button::LEFT_BUMPER, moonlight::LEFT_BUMPER),
    (button::RIGHT_BUMPER, moonlight::RIGHT_BUMPER),
    (button::BACK, moonlight::BACK),
    (button::START, moonlight::PLAY),
    (button::LEFT_STICK, moonlight::LEFT_STICK),
    (button::RIGHT_STICK, moonlight::RIGHT_STICK),
    (button::DPAD_UP, moonlight::UP),
    (button::DPAD_DOWN, moonlight::DOWN),
    (button::DPAD_LEFT, moonlight::LEFT),
    (button::DPAD_RIGHT, moonlight::RIGHT),
    (button::GUIDE, moonlight::SPECIAL),
];

/// The buttons that act as mouse buttons in mouse mode, with the Moonlight
/// mouse button each stands for: left, right, middle, X1 (back), X2 (forward).
const MOUSE_BUTTONS: [(u32, u8); 5] = [
    (button::A, 1),
    (button::B, 3),
    (button::X, 2),
    (button::LEFT_BUMPER, 4),
    (button::RIGHT_BUMPER, 5),
];
const MOUSE_BUTTON_MASK: u32 =
    button::A | button::B | button::X | button::LEFT_BUMPER | button::RIGHT_BUMPER;

/// A `gamepad-connected` message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    pub controller_id: u32,
    pub id: String,
    pub mapping: String,
    /// Button count of the Gamepad API object.
    pub buttons: u64,
    #[serde(default)]
    pub axes: u64,
    #[serde(default)]
    pub dual_rumble: bool,
    #[serde(default)]
    pub trigger_rumble: bool,
}

/// A `gamepad-state` snapshot in the client's units.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub controller_id: u32,
    pub seq: u64,
    pub buttons: u64,
    pub left_trigger: f64,
    pub right_trigger: f64,
    pub left_stick_x: f64,
    pub left_stick_y: f64,
    pub right_stick_x: f64,
    pub right_stick_y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GamepadMessage {
    Connected(Connected),
    Disconnected { controller_id: u32 },
    State(State),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Disconnect {
    controller_id: u32,
}

const TYPES: [&str; 3] = ["gamepad-connected", "gamepad-disconnected", "gamepad-state"];

/// Parse a message from the `gamepad` or control DataChannel.
pub fn parse(text: &str) -> Result<GamepadMessage, MessageError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| MessageError::Malformed(e.to_string()))?;
    check_envelope(&value, &TYPES, &ACCEPTED_VERSIONS)?;
    let malformed = |e: serde_json::Error| MessageError::Malformed(e.to_string());
    let kind = value["type"].as_str().unwrap_or_default().to_owned();
    match kind.as_str() {
        "gamepad-connected" => {
            let connected: Connected = serde_json::from_value(value).map_err(malformed)?;
            if connected.id.len() > 512 || connected.mapping.len() > 128 {
                return Err(MessageError::Invalid("controller id or mapping too long"));
            }
            if connected.buttons > 64 || connected.axes > 16 {
                return Err(MessageError::Invalid("too many buttons or axes"));
            }
            Ok(GamepadMessage::Connected(connected))
        }
        "gamepad-disconnected" => {
            let Disconnect { controller_id } = serde_json::from_value(value).map_err(malformed)?;
            Ok(GamepadMessage::Disconnected { controller_id })
        }
        _ => {
            let state: State = serde_json::from_value(value).map_err(malformed)?;
            if state.buttons > u64::from(button::ALL) {
                return Err(MessageError::Invalid("unknown button bits"));
            }
            Ok(GamepadMessage::State(state))
        }
    }
}

/// Moonlight button flags for a protocol button mask.
pub fn map_buttons(protocol: u32) -> u32 {
    BUTTON_MAP
        .iter()
        .filter(|(from, _)| protocol & from != 0)
        .fold(0, |flags, (_, to)| flags | to)
}

/// A trigger, 0..1, as 0..255.
pub fn map_trigger(value: f64) -> u8 {
    (value.clamp(0., 1.) * 255.).round() as u8
}

/// A stick axis, -1..1, as an `i16`. The Gamepad API's Y is down positive and
/// Moonlight's is up positive, so Y is inverted.
pub fn map_stick(value: f64, invert: bool) -> i16 {
    let value = value.clamp(-1., 1.);
    ((if invert { -value } else { value }) * 32767.).round() as i16
}

/// The Moonlight buttons a controller with `count` Gamepad API buttons has,
/// for the arrival packet.
pub fn supported_buttons(count: u64) -> u32 {
    // Gamepad API index and Moonlight flag. Indices 6 and 7 are the triggers.
    const BY_INDEX: [(u64, u32); 15] = [
        (0, moonlight::A),
        (1, moonlight::B),
        (2, moonlight::X),
        (3, moonlight::Y),
        (4, moonlight::LEFT_BUMPER),
        (5, moonlight::RIGHT_BUMPER),
        (8, moonlight::BACK),
        (9, moonlight::PLAY),
        (10, moonlight::LEFT_STICK),
        (11, moonlight::RIGHT_STICK),
        (12, moonlight::UP),
        (13, moonlight::DOWN),
        (14, moonlight::LEFT),
        (15, moonlight::RIGHT),
        (16, moonlight::SPECIAL),
    ];
    BY_INDEX
        .iter()
        .filter(|(index, _)| count > *index)
        .fold(0, |flags, (_, flag)| flags | flag)
}

/// Pointer motion for one mouse-mode tick from Moonlight-mapped sticks
/// `[left x, left y, right x, right y]` (Y up positive): whichever stick is
/// pushed further drives it, on a cubic curve with a deadzone.
///
/// The reference subtracts its deadzone from negative values too, so left and
/// up move faster than right and down. This version is symmetric.
pub fn mouse_delta(sticks: [i16; 4]) -> (i16, i16) {
    let [lx, ly, rx, ry] = sticks.map(i32::from);
    // Screen Y is down positive.
    let (ly, ry) = (-ly, -ry);
    let (x, y) = if lx.abs() + ly.abs() > rx.abs() + ry.abs() {
        (lx, ly)
    } else {
        (rx, ry)
    };
    let curve = |raw: i32| {
        let delta = (f64::from(raw) / 32766. * MOUSE_MOTION_MULTIPLIER).powi(3);
        let delta = if delta.abs() > MOUSE_DEADZONE {
            delta - MOUSE_DEADZONE.copysign(delta)
        } else {
            0.
        };
        delta
            .round()
            .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
    };
    (curve(x), curve(y))
}

/// A rumble motor speed, 0..65535, as 0..1.
pub fn rumble_magnitude(value: u16) -> f64 {
    f64::from(value) / 65535.
}

/// A message for the client's control DataChannel.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    MouseMode {
        controller_id: u32,
        slot: u8,
        active: bool,
    },
    /// All four motors, 0..1. Strong is the host's low-frequency motor.
    Rumble {
        controller_id: u32,
        slot: u8,
        strong: f64,
        weak: f64,
        left_trigger: f64,
        right_trigger: f64,
    },
}

impl Control {
    pub fn to_json(&self) -> String {
        match self {
            Self::MouseMode {
                controller_id,
                slot,
                active,
            } => json!({
                "v": CONTROL_VERSION,
                "type": "mouse-mode",
                "controllerId": controller_id,
                "controllerSlot": slot,
                "active": active,
            }),
            Self::Rumble {
                controller_id,
                slot,
                strong,
                weak,
                left_trigger,
                right_trigger,
            } => json!({
                "v": CONTROL_VERSION,
                "type": "rumble",
                "controllerId": controller_id,
                "controllerSlot": slot,
                "strongMagnitude": strong,
                "weakMagnitude": weak,
                "leftTrigger": left_trigger,
                "rightTrigger": right_trigger,
            }),
        }
        .to_string()
    }
}

/// What a gamepad event asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Inject into the input dispatcher, as if from the ENet control path.
    Input(Input),
    /// Send on the client's control DataChannel.
    Control(Control),
}

/// What handling one message did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    /// The message was valid and current. Stale snapshots, unknown
    /// controllers and a full slot table are not.
    pub accepted: bool,
    pub actions: Vec<Action>,
}

/// A snapshot in Moonlight's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Pad {
    buttons: u32,
    left_trigger: u8,
    right_trigger: u8,
    /// Left X, left Y, right X, right Y; Y up positive.
    sticks: [i16; 4],
}

#[derive(Debug)]
struct Controller {
    /// What the client announced, to tell a repeat of it from a real change.
    announced: Connected,
    slot: u8,
    capabilities: u16,
    last_seq: Option<u64>,
    stale_states: u64,
    sequence_gaps: u64,
    /// The latest accepted snapshot, which mouse-mode ticks read.
    latest: Pad,
    /// The last state sent to the host and the mask it carried.
    last_sent: Option<(u16, Pad)>,
    /// Protocol button mask of the previous accepted snapshot.
    previous_buttons: u32,
    start_pressed_at: Option<Instant>,
    mouse_mode: bool,
    /// Mouse-mode buttons held when the mode turned on, ignored until released.
    suppressed: u32,
    /// Protocol bits of the mouse buttons this controller holds down.
    simulated: u32,
    /// Strong, weak, left trigger, right trigger.
    rumble: [f64; 4],
}

/// The gamepads of one WebRTC client.
#[derive(Debug, Default)]
pub struct Gamepads {
    controllers: BTreeMap<u32, Controller>,
    slots: [Option<u32>; MAX_GAMEPADS],
    active_mask: u16,
    next_tick: Option<Instant>,
}

impl Gamepads {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moonlight's active gamepad mask.
    pub fn active_mask(&self) -> u16 {
        self.active_mask
    }

    pub fn slot(&self, controller_id: u32) -> Option<u8> {
        self.controllers.get(&controller_id).map(|c| c.slot)
    }

    pub fn mouse_mode(&self, controller_id: u32) -> bool {
        self.controllers
            .get(&controller_id)
            .is_some_and(|c| c.mouse_mode)
    }

    /// Snapshots dropped as out of order, for the stream summary.
    pub fn stale_states(&self, controller_id: u32) -> u64 {
        self.controllers
            .get(&controller_id)
            .map_or(0, |c| c.stale_states)
    }

    /// Snapshots the client sent that never arrived.
    pub fn sequence_gaps(&self, controller_id: u32) -> u64 {
        self.controllers
            .get(&controller_id)
            .map_or(0, |c| c.sequence_gaps)
    }

    /// The time the next [`tick`](Self::tick) is due, while any controller is
    /// in mouse mode.
    pub fn next_tick(&self) -> Option<Instant> {
        self.next_tick
    }

    /// Parse and apply one gamepad message.
    pub fn handle_message(&mut self, text: &str, now: Instant) -> Result<Outcome, MessageError> {
        Ok(match parse(text)? {
            GamepadMessage::Connected(connected) => self.connect(&connected),
            GamepadMessage::Disconnected { controller_id } => self.disconnect(controller_id),
            GamepadMessage::State(state) => self.state(&state, now),
        })
    }

    /// A controller appears. It gets the lowest free slot and an arrival
    /// event; a controller that reconnects under the same ID with a different
    /// description is replaced. The same announcement again changes nothing: a
    /// client that announces on both channels must not replug the pad.
    pub fn connect(&mut self, connected: &Connected) -> Outcome {
        if self
            .controllers
            .get(&connected.controller_id)
            .is_some_and(|known| known.announced == *connected)
        {
            return Outcome {
                accepted: true,
                actions: Vec::new(),
            };
        }
        let mut actions = Vec::new();
        self.remove(connected.controller_id, &mut actions);
        let Some(slot) = self.slots.iter().position(Option::is_none) else {
            return Outcome {
                accepted: false,
                actions,
            };
        };
        let mut capabilities = 0;
        if connected.buttons > 7 {
            capabilities |= CCAP_ANALOG_TRIGGERS;
        }
        if connected.dual_rumble {
            capabilities |= CCAP_RUMBLE;
        }
        if connected.trigger_rumble {
            capabilities |= CCAP_TRIGGER_RUMBLE;
        }
        self.slots[slot] = Some(connected.controller_id);
        self.active_mask |= 1 << slot;
        self.controllers.insert(
            connected.controller_id,
            Controller {
                announced: connected.clone(),
                slot: slot as u8,
                capabilities,
                last_seq: None,
                stale_states: 0,
                sequence_gaps: 0,
                latest: Pad::default(),
                last_sent: None,
                previous_buttons: 0,
                start_pressed_at: None,
                mouse_mode: false,
                suppressed: 0,
                simulated: 0,
                rumble: [0.; 4],
            },
        );
        actions.push(Action::Input(Input::Arrival {
            id: slot as u8,
            kind: CTYPE_XBOX,
            capabilities,
            buttons: supported_buttons(connected.buttons),
        }));
        Outcome {
            accepted: true,
            actions,
        }
    }

    /// A controller goes away: its mouse buttons are released, its rumble
    /// stops, and the host sees a neutral state and then its bit leave the
    /// active mask.
    pub fn disconnect(&mut self, controller_id: u32) -> Outcome {
        let mut actions = Vec::new();
        let accepted = self.remove(controller_id, &mut actions);
        Outcome { accepted, actions }
    }

    /// The stream ended or the channels closed: every controller goes away.
    pub fn close(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        let ids: Vec<u32> = self.controllers.keys().copied().collect();
        for id in ids {
            self.remove(id, &mut actions);
        }
        self.active_mask = 0;
        self.next_tick = None;
        actions
    }

    /// Apply a `gamepad-state` snapshot received at `now`.
    pub fn state(&mut self, state: &State, now: Instant) -> Outcome {
        let mask = self.active_mask;
        let Some(c) = self.controllers.get_mut(&state.controller_id) else {
            return Outcome::default();
        };
        // The channel is unordered and unreliable: an older snapshot must not
        // undo a newer one.
        if c.last_seq.is_some_and(|last| state.seq <= last) {
            c.stale_states += 1;
            return Outcome::default();
        }
        if let Some(last) = c.last_seq {
            c.sequence_gaps += state.seq - last - 1;
        }
        c.last_seq = Some(state.seq);

        let buttons = state.buttons as u32 & button::ALL;
        let pad = Pad {
            buttons: map_buttons(buttons),
            left_trigger: map_trigger(state.left_trigger),
            right_trigger: map_trigger(state.right_trigger),
            sticks: [
                map_stick(state.left_stick_x, false),
                map_stick(state.left_stick_y, true),
                map_stick(state.right_stick_x, false),
                map_stick(state.right_stick_y, true),
            ],
        };
        c.latest = pad;
        let mut actions = Vec::new();

        let start = buttons & button::START != 0;
        let was_start = c.previous_buttons & button::START != 0;
        if start && !was_start {
            c.start_pressed_at = Some(now);
        }
        let mut toggled = false;
        if !start
            && was_start
            && c.start_pressed_at
                .is_some_and(|at| now.saturating_duration_since(at) >= MOUSE_MODE_HOLD)
        {
            toggled = true;
            if c.mouse_mode {
                release_simulated(c, &mut actions);
                c.mouse_mode = false;
                c.suppressed = 0;
            } else {
                // The game sees a neutral pad from now on, not the buttons
                // that happened to be held when the mode turned on.
                send_pad(c, mask, Pad::default(), &mut actions);
                c.mouse_mode = true;
                c.suppressed = buttons & MOUSE_BUTTON_MASK;
                self.next_tick.get_or_insert(now + MOUSE_TICK);
            }
            actions.push(Action::Control(Control::MouseMode {
                controller_id: state.controller_id,
                slot: c.slot,
                active: c.mouse_mode,
            }));
            if !c.mouse_mode && !self.controllers_in_mouse_mode() {
                self.next_tick = None;
            }
        }
        // `c` ended its borrow above; look it up again after touching `self`.
        let c = self
            .controllers
            .get_mut(&state.controller_id)
            .expect("the controller was just found");
        if !start {
            c.start_pressed_at = None;
        }

        if c.mouse_mode {
            let effective = buttons & !c.suppressed;
            let previous = c.previous_buttons & !c.suppressed;
            if !toggled {
                for (bit, mouse_button) in MOUSE_BUTTONS {
                    let pressed = effective & bit != 0;
                    if pressed != (previous & bit != 0) {
                        actions.push(Action::Input(Input::MouseButton {
                            button: mouse_button,
                            down: pressed,
                        }));
                    }
                }
                let rising = |bit: u32| buttons & bit != 0 && c.previous_buttons & bit == 0;
                for (bit, amount, horizontal) in [
                    (button::DPAD_UP, WHEEL_NOTCH, false),
                    (button::DPAD_DOWN, -WHEEL_NOTCH, false),
                    (button::DPAD_RIGHT, WHEEL_NOTCH, true),
                    (button::DPAD_LEFT, -WHEEL_NOTCH, true),
                ] {
                    if rising(bit) {
                        actions.push(Action::Input(Input::Scroll { amount, horizontal }));
                    }
                }
            }
            c.suppressed &= buttons;
            c.simulated = effective & MOUSE_BUTTON_MASK;
        } else {
            send_pad(c, mask, pad, &mut actions);
        }
        c.previous_buttons = buttons;
        Outcome {
            accepted: true,
            actions,
        }
    }

    /// Pointer motion for every controller in mouse mode, once per
    /// [`MOUSE_TICK`]. Call it whenever the clock may have reached
    /// [`next_tick`](Self::next_tick); earlier calls do nothing.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let Some(due) = self.next_tick else {
            return Vec::new();
        };
        if now < due {
            return Vec::new();
        }
        self.next_tick = Some(now + MOUSE_TICK);
        self.controllers
            .values()
            .filter(|c| c.mouse_mode)
            .filter_map(|c| {
                let (x, y) = mouse_delta(c.latest.sticks);
                (x != 0 || y != 0).then_some(Action::Input(Input::Relative { x, y }))
            })
            .collect()
    }

    /// Host rumble for a Moonlight slot, as the `rumble` message. `low` and
    /// `high` are the host's low- and high-frequency motors, 0..65535. None
    /// when the slot is empty or its controller announced no rumble.
    pub fn rumble(&mut self, slot: u16, low: u16, high: u16) -> Option<Control> {
        let (id, c) = self.by_slot(slot)?;
        if c.capabilities & CCAP_RUMBLE == 0 {
            return None;
        }
        c.rumble[0] = rumble_magnitude(low);
        c.rumble[1] = rumble_magnitude(high);
        Some(rumble_control(id, c))
    }

    /// Host trigger rumble for a Moonlight slot, as the `rumble` message.
    pub fn trigger_rumble(&mut self, slot: u16, left: u16, right: u16) -> Option<Control> {
        let (id, c) = self.by_slot(slot)?;
        if c.capabilities & CCAP_TRIGGER_RUMBLE == 0 {
            return None;
        }
        c.rumble[2] = rumble_magnitude(left);
        c.rumble[3] = rumble_magnitude(right);
        Some(rumble_control(id, c))
    }

    fn by_slot(&mut self, slot: u16) -> Option<(u32, &mut Controller)> {
        let id = (*self.slots.get(usize::from(slot))?)?;
        Some((id, self.controllers.get_mut(&id)?))
    }

    fn controllers_in_mouse_mode(&self) -> bool {
        self.controllers.values().any(|c| c.mouse_mode)
    }

    /// Release everything a controller holds and drop it. False when there
    /// was no such controller.
    fn remove(&mut self, controller_id: u32, actions: &mut Vec<Action>) -> bool {
        let Some(mut c) = self.controllers.remove(&controller_id) else {
            return false;
        };
        release_simulated(&mut c, actions);
        if c.mouse_mode {
            actions.push(Action::Control(Control::MouseMode {
                controller_id,
                slot: c.slot,
                active: false,
            }));
        }
        if c.rumble != [0.; 4] {
            c.rumble = [0.; 4];
            actions.push(Action::Control(rumble_control(controller_id, &c)));
        }
        // Release everything first, then drop the slot from the mask so the
        // host unplugs the pad.
        actions.push(Action::Input(controller_input(
            c.slot,
            self.active_mask,
            Pad::default(),
        )));
        self.active_mask &= !(1 << c.slot);
        actions.push(Action::Input(controller_input(
            c.slot,
            self.active_mask,
            Pad::default(),
        )));
        self.slots[usize::from(c.slot)] = None;
        if !self.controllers_in_mouse_mode() {
            self.next_tick = None;
        }
        true
    }
}

fn rumble_control(controller_id: u32, c: &Controller) -> Control {
    Control::Rumble {
        controller_id,
        slot: c.slot,
        strong: c.rumble[0],
        weak: c.rumble[1],
        left_trigger: c.rumble[2],
        right_trigger: c.rumble[3],
    }
}

fn controller_input(slot: u8, active: u16, pad: Pad) -> Input {
    Input::Controller {
        id: u16::from(slot),
        active,
        buttons: pad.buttons,
        left_trigger: pad.left_trigger,
        right_trigger: pad.right_trigger,
        sticks: pad.sticks,
    }
}

/// Send `pad` unless the host already has exactly this state (a keepalive).
fn send_pad(c: &mut Controller, active: u16, pad: Pad, actions: &mut Vec<Action>) {
    if c.last_sent != Some((active, pad)) {
        c.last_sent = Some((active, pad));
        actions.push(Action::Input(controller_input(c.slot, active, pad)));
    }
}

fn release_simulated(c: &mut Controller, actions: &mut Vec<Action>) {
    for (bit, mouse_button) in MOUSE_BUTTONS {
        if c.simulated & bit != 0 {
            actions.push(Action::Input(Input::MouseButton {
                button: mouse_button,
                down: false,
            }));
        }
    }
    c.simulated = 0;
    c.suppressed = 0;
}

#[cfg(test)]
mod tests;
