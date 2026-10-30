//! The input queue: the `LiSend*` API, with moonlight-common-c's batching (relative motion
//! summed, absolute motion, controller axes and motion sensors sent as their latest state,
//! pen moves collapsed, text split per code point) and Nightjar's network grace rule
//! (input held back while the host is silent, then replaced by the current state).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rubylight_protocol::pen;

use super::packets::{
    button, capability, motion_type, touch_event, touchpad, InputEvent, TouchpadContact,
    MAX_CONTROLLERS,
};
use crate::control::negotiation::{feature, HostCapabilities};
use crate::control::wire::Delivery;

/// Most packets waiting to be sent (`MAX_QUEUED_INPUT_PACKETS`).
pub const MAX_QUEUED: usize = 150;
/// Mouse and pen motion closer together than this waits to be batched with the next.
pub const MOTION_BATCHING_INTERVAL: Duration = Duration::from_millis(1);
/// Text waits this long after earlier input so Windows has processed key events first.
pub const TEXT_SETTLE: Duration = Duration::from_millis(50);
const WHEEL_DELTA: i32 = 120;
const MAX_MOTION_TYPES: usize = 2;
const HELD_KEYS: usize = 256;
const HELD_MOUSE_BUTTONS: usize = 8;

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

#[derive(Clone, Copy, Default)]
struct SensorState {
    xyz: [f32; 3],
    dirty: bool,
}

/// The latest state of what the host must not lose while it is silent.
struct Held {
    keyboard: Vec<Option<Pending>>,
    mouse_buttons: [Option<Pending>; HELD_MOUSE_BUTTONS],
    controllers: [Option<Pending>; MAX_CONTROLLERS as usize],
    arrivals: [Option<Pending>; MAX_CONTROLLERS as usize],
    motion: [[Option<Pending>; MAX_MOTION_TYPES]; MAX_CONTROLLERS as usize],
    batteries: [Option<Pending>; MAX_CONTROLLERS as usize],
    touch: Option<Pending>,
    pen: Option<Pending>,
    dropped: u32,
}

impl Default for Held {
    fn default() -> Self {
        Self {
            keyboard: vec![None; HELD_KEYS],
            mouse_buttons: Default::default(),
            controllers: Default::default(),
            arrivals: Default::default(),
            motion: Default::default(),
            batteries: Default::default(),
            touch: None,
            pen: None,
            dropped: 0,
        }
    }
}

/// The input stream of one connection. Producers call the `send_*` methods from any thread
/// (behind a lock); the control thread calls [`Self::poll`] and sends what it returns.
pub struct InputQueue {
    sunshine: bool,
    feature_flags: u32,
    batched_scroll: bool,
    stopped: bool,
    unbatched: bool,
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
    scroll_accumulator: i32,
    position: (f32, f32),
    last_mouse: Option<Instant>,
    last_pen: Option<Instant>,
    text_ready_at: Option<Instant>,
    holding: bool,
    held: Held,
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
            scroll_accumulator: 0,
            position: (0.5, 0.5),
            last_mouse: None,
            last_pen: None,
            text_ready_at: None,
            holding: false,
            held: Held::default(),
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

    /// Sends a pen's known barrel roll as 0x5500000B (`rubylight_protocol::pen`). Only for a
    /// host with both `PenBarrelRollVersion` and `x-ss-general.penBarrelRoll`; otherwise the
    /// roll is dropped and pens go as the stock 0x55000003.
    pub fn set_pen_barrel_roll(&mut self, offered: bool) {
        self.pen_barrel_roll = offered;
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
            return Err(InputError::QueueFull);
        }
        self.entries.push_back(entry);
        Ok(())
    }

    fn offer_event(&mut self, event: InputEvent) -> Result<(), InputError> {
        self.offer(Entry::Event(Pending::new(event)))
    }

    pub fn send_mouse_move(&mut self, dx: i16, dy: i16) -> Result<(), InputError> {
        self.check()?;
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        self.relative.0 += i32::from(dx);
        self.relative.1 += i32::from(dy);
        if !self.relative_dirty {
            self.offer(Entry::RelativeMouse)?;
            self.relative_dirty = true;
        }
        Ok(())
    }

    /// An absolute position within a `width` x `height` reference area.
    pub fn send_mouse_position(
        &mut self,
        x: i16,
        y: i16,
        width: i16,
        height: i16,
    ) -> Result<(), InputError> {
        self.check()?;
        self.absolute = (x, y, width, height);
        let result = if self.absolute_dirty {
            Ok(())
        } else {
            self.offer(Entry::AbsoluteMouse)
                .inspect(|_| self.absolute_dirty = true)
        };
        let fraction = |v: i16, size: i16| {
            let span = f32::from(size) - 1.0;
            if span <= 0.0 {
                0.0
            } else {
                f32::from(v).clamp(0.0, span) / span
            }
        };
        self.position = (fraction(x, width), fraction(y, height));
        result
    }

    /// Relative motion sent as an absolute position, from the last position sent.
    pub fn send_mouse_move_as_position(
        &mut self,
        dx: i16,
        dy: i16,
        width: i16,
        height: i16,
    ) -> Result<(), InputError> {
        let old_x = (self.position.0 * (f32::from(width) - 1.0) + 0.5) as i16;
        let old_y = (self.position.1 * (f32::from(height) - 1.0) + 0.5) as i16;
        let clamp = |v: i32, max: i16| v.clamp(0, i32::from(max.max(0))) as i16;
        self.send_mouse_position(
            clamp(i32::from(old_x) + i32::from(dx), width),
            clamp(i32::from(old_y) + i32::from(dy), height),
            width,
            height,
        )
    }

    pub fn send_mouse_button(&mut self, button: u8, down: bool) -> Result<(), InputError> {
        self.check()?;
        self.offer_event(InputEvent::MouseButton { button, down })
    }

    /// A key with a Windows virtual key code. GFE gets the modifier fix-ups the C applies;
    /// `flags` reaches Sunshine hosts only.
    pub fn send_keyboard(
        &mut self,
        key_code: u16,
        down: bool,
        mut modifiers: u8,
        flags: u8,
    ) -> Result<(), InputError> {
        use super::packets::modifier::*;
        self.check()?;
        if !self.sunshine {
            match key_code & 0xFF {
                // GFE drops events carrying META, but the Windows key itself works without it.
                0x5B | 0x5C => modifiers &= !META,
                0xA0 => modifiers |= SHIFT,
                0xA1 => modifiers &= !SHIFT,
                0xA2 => modifiers |= CTRL,
                0xA3 => modifiers &= !CTRL,
                0xA4 => modifiers |= ALT,
                0xA5 => modifiers &= !ALT,
                _ => {}
            }
            modifiers &= !EXTENDED;
        }
        let flags = if self.sunshine { flags } else { 0 };
        self.offer_event(InputEvent::Keyboard {
            key_code,
            down,
            modifiers,
            flags,
        })
    }

    pub fn send_text(&mut self, text: &str) -> Result<(), InputError> {
        self.check()?;
        if text.is_empty() {
            return Ok(());
        }
        self.offer(Entry::Text(text.as_bytes().to_vec()))
    }

    /// A controller's full state. Calls with unchanged buttons update a state still queued.
    #[allow(clippy::too_many_arguments)]
    pub fn send_controller(
        &mut self,
        number: u16,
        mut active_mask: u16,
        mut buttons: u32,
        left_trigger: u8,
        right_trigger: u8,
        left_stick: (i16, i16),
        right_stick: (i16, i16),
    ) -> Result<(), InputError> {
        self.check()?;
        // Callers that passed a sign-extended 16-bit value set every upper flag with Y.
        if buttons & 0x8000_0000 != 0 {
            buttons &= 0xFFFF;
        }
        let number = if self.sunshine {
            number % u16::from(MAX_CONTROLLERS)
        } else {
            // GFE takes four controllers and no extended buttons; MISC stands in for Guide.
            active_mask &= 0xF;
            if buttons & button::MISC != 0 {
                buttons |= button::SPECIAL;
            }
            number % 4
        };
        let event = InputEvent::Controller {
            number,
            active_mask,
            buttons,
            left_trigger,
            right_trigger,
            left_stick,
            right_stick,
        };
        let sunshine = self.sunshine;
        let same_buttons = |queued: &InputEvent| match queued {
            InputEvent::Controller { buttons: old, .. } => {
                *old as u16 == buttons as u16
                    && (!sunshine || (*old >> 16) as u16 == (buttons >> 16) as u16)
            }
            _ => false,
        };
        if let Some(id) = self.controller_batch[usize::from(number)] {
            if let Some(Entry::Controller { state, .. }) = self
                .entries
                .iter_mut()
                .rev()
                .find(|e| matches!(e, Entry::Controller { id: queued, .. } if *queued == id))
            {
                if same_buttons(&state.event) {
                    state.event = event;
                    return Ok(());
                }
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.offer(Entry::Controller {
            id,
            state: Pending::new(event),
        })?;
        self.controller_batch[usize::from(number)] = Some(id);
        Ok(())
    }

    /// `LiSendControllerEvent`: controller 0 alone.
    pub fn send_single_controller(
        &mut self,
        buttons: u32,
        left_trigger: u8,
        right_trigger: u8,
        left_stick: (i16, i16),
        right_stick: (i16, i16),
    ) -> Result<(), InputError> {
        self.send_controller(
            0,
            1,
            buttons,
            left_trigger,
            right_trigger,
            left_stick,
            right_stick,
        )
    }

    /// Announces a controller to Sunshine hosts, then sends a neutral state for hosts that do
    /// not take arrivals.
    pub fn send_controller_arrival(
        &mut self,
        number: u8,
        active_mask: u16,
        kind: u8,
        supported_buttons: u32,
        mut capabilities: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        let number = number % MAX_CONTROLLERS;
        if capabilities & capability::DUAL_TOUCHPAD != 0 {
            capabilities |= capability::TOUCHPAD;
        }
        if self.sunshine {
            self.offer_event(InputEvent::ControllerArrival {
                number,
                kind,
                capabilities,
                supported_buttons,
            })?;
        }
        self.send_controller(u16::from(number), active_mask, 0, 0, 0, (0, 0), (0, 0))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_controller_touch(
        &mut self,
        number: u8,
        event: u8,
        touchpad: u8,
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure: f32,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::CONTROLLER_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        let number = number % MAX_CONTROLLERS;
        self.offer_event(InputEvent::ControllerTouch {
            number,
            event,
            touchpad,
            pointer_id,
            x,
            y,
            pressure,
        })
    }

    /// Accelerometer (m/s², gravity included) or gyroscope (deg/s); only the latest value per
    /// controller and sensor is sent.
    pub fn send_controller_motion(
        &mut self,
        number: u8,
        kind: u8,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<(), InputError> {
        self.check()?;
        if !(1..=MAX_MOTION_TYPES as u8).contains(&kind) {
            return Err(InputError::Invalid);
        }
        if self.feature_flags & feature::CONTROLLER_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        let number = number % MAX_CONTROLLERS;
        let sensor = &mut self.sensors[usize::from(number)][usize::from(kind - 1)];
        sensor.xyz = [x, y, z];
        if !sensor.dirty {
            self.offer(Entry::Motion { number, kind })?;
            self.sensors[usize::from(number)][usize::from(kind - 1)].dirty = true;
        }
        Ok(())
    }

    pub fn send_controller_battery(
        &mut self,
        number: u8,
        state: u8,
        percentage: u8,
    ) -> Result<(), InputError> {
        self.check()?;
        if !self.sunshine {
            return Err(InputError::Unsupported);
        }
        let number = number % MAX_CONTROLLERS;
        self.offer_event(InputEvent::ControllerBattery {
            number,
            state,
            percentage,
        })
    }

    /// Whole wheel clicks.
    pub fn send_scroll(&mut self, clicks: i8) -> Result<(), InputError> {
        self.send_high_res_scroll(i16::from(clicks) * WHEEL_DELTA as i16)
    }

    /// Scroll in 1/120 of a click. GFE's virtual HID devices take only whole clicks, so for
    /// them amounts are gathered into clicks.
    pub fn send_high_res_scroll(&mut self, amount: i16) -> Result<(), InputError> {
        self.check()?;
        if amount == 0 {
            return Ok(());
        }
        if !self.batched_scroll {
            return self.offer_event(InputEvent::Scroll { amount });
        }
        if (self.scroll_accumulator < 0 && amount > 0)
            || (self.scroll_accumulator > 0 && amount < 0)
        {
            self.scroll_accumulator = 0;
        }
        self.scroll_accumulator += i32::from(amount);
        while self.scroll_accumulator.abs() >= WHEEL_DELTA {
            let click = if self.scroll_accumulator > 0 {
                WHEEL_DELTA
            } else {
                -WHEEL_DELTA
            };
            self.offer_event(InputEvent::Scroll {
                amount: click as i16,
            })?;
            self.scroll_accumulator -= click;
        }
        Ok(())
    }

    pub fn send_hscroll(&mut self, clicks: i8) -> Result<(), InputError> {
        self.send_high_res_hscroll(i16::from(clicks) * WHEEL_DELTA as i16)
    }

    pub fn send_high_res_hscroll(&mut self, amount: i16) -> Result<(), InputError> {
        self.check()?;
        if !self.sunshine {
            return Err(InputError::Unsupported);
        }
        if amount == 0 {
            return Ok(());
        }
        self.offer_event(InputEvent::HScroll { amount })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_touch(
        &mut self,
        event: u8,
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure_or_distance: f32,
        contact_major: f32,
        contact_minor: f32,
        rotation: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::PEN_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        self.offer_event(InputEvent::Touch {
            event,
            pointer_id,
            x,
            y,
            pressure_or_distance,
            contact_major,
            contact_minor,
            rotation,
        })
    }

    /// `rotation` is the tilt's direction (stock), `roll` the barrel roll in degrees 0-359
    /// clockwise or [`pen::ROLL_UNKNOWN`]; the roll goes only to a host that takes it.
    #[allow(clippy::too_many_arguments)]
    pub fn send_pen(
        &mut self,
        event: u8,
        tool: u8,
        buttons: u8,
        x: f32,
        y: f32,
        pressure_or_distance: f32,
        contact_major: f32,
        contact_minor: f32,
        rotation: u16,
        tilt: u8,
        roll: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::PEN_TOUCH_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        // Moves and hovers without a button change may be dropped for a newer one.
        let delivery = if touch_event::is_batchable(event) && buttons == self.pen_buttons {
            Delivery::Unreliable
        } else {
            Delivery::Reliable
        };
        self.pen_buttons = buttons;
        let event = InputEvent::Pen {
            event,
            tool,
            buttons,
            x,
            y,
            pressure_or_distance,
            contact_major,
            contact_minor,
            rotation,
            tilt,
            roll: if self.pen_barrel_roll && roll < 360 {
                roll
            } else {
                pen::ROLL_UNKNOWN
            },
        };
        self.offer(Entry::Event(Pending {
            channel: event.channel(),
            delivery,
            event,
        }))
    }

    /// `LiSendTouchpadFrameEvent`: the contacts of one laptop touchpad frame, at most
    /// [`touchpad::MAX_FRAME_CONTACTS`]. A host without `TOUCHPAD_FRAME_EVENTS` refuses it.
    pub fn send_touchpad_frame(
        &mut self,
        contacts: &[TouchpadContact],
        buttons: u8,
        width_mm: u16,
        height_mm: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::TOUCHPAD_FRAME_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        if contacts.len() > touchpad::MAX_FRAME_CONTACTS {
            return Err(InputError::Invalid);
        }
        let mut frame = [TouchpadContact::default(); touchpad::MAX_FRAME_CONTACTS];
        frame[..contacts.len()].copy_from_slice(contacts);
        // Only a frame of moves with no button change may be dropped for a newer one.
        let batchable = contacts.iter().all(|c| touch_event::is_batchable(c.event));
        let delivery = self.touchpad_delivery(batchable, buttons);
        let event = InputEvent::TouchpadFrame {
            count: contacts.len() as u8,
            buttons,
            rotation: touchpad::ROTATION_UNKNOWN,
            width_mm,
            height_mm,
            contacts: frame,
        };
        self.offer(Entry::Event(Pending { channel: event.channel(), delivery, event }))
    }

    /// `LiSendTouchpadEvent`: one laptop touchpad contact. A host without `TOUCHPAD_EVENTS`
    /// refuses it.
    pub fn send_touchpad(
        &mut self,
        contact: TouchpadContact,
        buttons: u8,
        width_mm: u16,
        height_mm: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if self.feature_flags & feature::TOUCHPAD_EVENTS == 0 {
            return Err(InputError::Unsupported);
        }
        let delivery = self.touchpad_delivery(touch_event::is_batchable(contact.event), buttons);
        let event = InputEvent::Touchpad {
            event: contact.event,
            buttons,
            rotation: touchpad::ROTATION_UNKNOWN,
            pointer_id: contact.pointer_id,
            width_mm,
            height_mm,
            x: contact.x,
            y: contact.y,
            pressure: contact.pressure,
            contact_major: 0.0,
            contact_minor: 0.0,
        };
        self.offer(Entry::Event(Pending { channel: event.channel(), delivery, event }))
    }

    /// Moves without a button change may be dropped; a click or release never is.
    fn touchpad_delivery(&mut self, batchable: bool, buttons: u8) -> Delivery {
        let delivery = if batchable && buttons == self.touchpad_buttons {
            Delivery::Unreliable
        } else {
            Delivery::Reliable
        };
        self.touchpad_buttons = buttons;
        delivery
    }

    /// Takes what is ready to send into `out`. `contact_lost` is the driver's view that the
    /// host has been silent through a network grace period. Returns when to call again for
    /// input waiting on a batching interval or the text settle time.
    pub fn poll(
        &mut self,
        now: Instant,
        contact_lost: bool,
        out: &mut Vec<InputPacket>,
    ) -> Option<Instant> {
        self.poll_inner(now, contact_lost, false, out)
    }

    /// Takes everything without waiting for batching intervals, as the C drains its queue
    /// when input stops.
    pub fn drain(&mut self, now: Instant, contact_lost: bool, out: &mut Vec<InputPacket>) {
        self.poll_inner(now, contact_lost, true, out);
    }

    fn poll_inner(
        &mut self,
        now: Instant,
        contact_lost: bool,
        force: bool,
        out: &mut Vec<InputPacket>,
    ) -> Option<Instant> {
        if self.holding && !contact_lost {
            self.release_held(out);
        }
        let motion_interval = self.motion_interval();
        while let Some(entry) = self.entries.front() {
            let wait_until = match entry {
                Entry::RelativeMouse | Entry::AbsoluteMouse => {
                    self.last_mouse.map(|t| t + motion_interval)
                }
                Entry::Event(Pending {
                    event: InputEvent::Pen { event, .. },
                    ..
                }) if touch_event::is_batchable(*event) => {
                    self.last_pen.map(|t| t + motion_interval)
                }
                Entry::Text(_) if !contact_lost => {
                    Some(*self.text_ready_at.get_or_insert(now + TEXT_SETTLE))
                }
                _ => None,
            };
            if let Some(at) = wait_until {
                if now < at && !force {
                    return Some(at);
                }
            }
            let entry = self.entries.pop_front().expect("front exists");
            match entry {
                Entry::Event(pending) => {
                    if let InputEvent::Pen { event, buttons, .. } = pending.event {
                        if touch_event::is_batchable(event) {
                            let latest = self.collapse_pen(pending, event, buttons);
                            self.last_pen = Some(now);
                            self.emit(latest, contact_lost, out);
                            continue;
                        }
                    }
                    self.emit(pending, contact_lost, out);
                }
                Entry::Controller { id, state } => {
                    if let InputEvent::Controller { number, .. } = state.event {
                        let slot = &mut self.controller_batch[usize::from(number)];
                        if *slot == Some(id) {
                            *slot = None;
                        }
                    }
                    self.emit(state, contact_lost, out);
                }
                Entry::RelativeMouse => {
                    let split = |remaining: &mut i32| {
                        let part = (*remaining).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                        *remaining -= part;
                        part as i16
                    };
                    while self.relative != (0, 0) {
                        let dx = split(&mut self.relative.0);
                        let dy = split(&mut self.relative.1);
                        self.emit(
                            Pending::new(InputEvent::MouseMove { dx, dy }),
                            contact_lost,
                            out,
                        );
                    }
                    self.relative_dirty = false;
                    self.last_mouse = Some(now);
                }
                Entry::AbsoluteMouse => {
                    let (x, y, width, height) = self.absolute;
                    // GFE's scaling cannot reach the far edge without the reference size less one.
                    let event = InputEvent::MousePosition {
                        x,
                        y,
                        width: width.wrapping_sub(1),
                        height: height.wrapping_sub(1),
                    };
                    self.absolute_dirty = false;
                    self.last_mouse = Some(now);
                    self.emit(Pending::new(event), contact_lost, out);
                }
                Entry::Motion { number, kind } => {
                    let sensor = &mut self.sensors[usize::from(number)][usize::from(kind - 1)];
                    sensor.dirty = false;
                    let [x, y, z] = sensor.xyz;
                    // A zero gyro reading lets a client park the gyro; it must not be lost.
                    let delivery = if kind == motion_type::GYRO && x == 0.0 && y == 0.0 && z == 0.0
                    {
                        Delivery::Reliable
                    } else {
                        Delivery::Unreliable
                    };
                    let event = InputEvent::ControllerMotion {
                        number,
                        motion_type: kind,
                        x,
                        y,
                        z,
                    };
                    self.emit(
                        Pending {
                            channel: event.channel(),
                            delivery,
                            event,
                        },
                        contact_lost,
                        out,
                    );
                }
                Entry::Text(text) => {
                    self.text_ready_at = None;
                    if contact_lost {
                        // Text typed while the host is silent would arrive late.
                        self.note_held();
                        continue;
                    }
                    for code_point in code_points(&text) {
                        self.emit(
                            Pending::new(InputEvent::Text(code_point.to_vec())),
                            contact_lost,
                            out,
                        );
                    }
                }
            }
        }
        None
    }

    /// Replaces a pen move or hover with the newest one queued right behind it with the same
    /// event and buttons.
    fn collapse_pen(&mut self, mut pending: Pending, event: u8, buttons: u8) -> Pending {
        while let Some(Entry::Event(Pending {
            event:
                InputEvent::Pen {
                    event: next_event,
                    buttons: next_buttons,
                    ..
                },
            ..
        })) = self.entries.front()
        {
            if *next_event != event || *next_buttons != buttons {
                break;
            }
            let Some(Entry::Event(next)) = self.entries.pop_front() else {
                unreachable!("front was a pen event");
            };
            pending = next;
        }
        pending
    }

    fn emit(&mut self, pending: Pending, contact_lost: bool, out: &mut Vec<InputPacket>) {
        if contact_lost {
            self.hold(pending);
            return;
        }
        out.push(InputPacket {
            data: pending.event.encode(self.sunshine),
            channel: pending.channel,
            delivery: pending.delivery,
        });
    }

    fn note_held(&mut self) {
        self.holding = true;
        self.held.dropped = self.held.dropped.wrapping_add(1);
    }

    fn hold(&mut self, mut pending: Pending) {
        self.note_held();
        let held = &mut self.held;
        match pending.event {
            InputEvent::Keyboard { key_code, .. } => {
                held.keyboard[usize::from(key_code & 0xFF)] = Some(pending)
            }
            InputEvent::MouseButton { button, .. } => {
                if let Some(slot) = held.mouse_buttons.get_mut(usize::from(button)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::Controller { number, .. } => {
                if let Some(slot) = held.controllers.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::ControllerArrival { number, .. } => {
                if let Some(slot) = held.arrivals.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            InputEvent::ControllerMotion {
                number,
                motion_type,
                ..
            } => {
                if (1..=MAX_MOTION_TYPES as u8).contains(&motion_type) {
                    if let Some(slots) = held.motion.get_mut(usize::from(number)) {
                        slots[usize::from(motion_type - 1)] = Some(pending);
                    }
                }
            }
            InputEvent::ControllerBattery { number, .. } => {
                if let Some(slot) = held.batteries.get_mut(usize::from(number)) {
                    *slot = Some(pending);
                }
            }
            // Contacts from the silence are cancelled once the host is back.
            InputEvent::Touch { ref mut event, .. } => {
                *event = touch_event::CANCEL_ALL;
                pending.delivery = Delivery::Reliable;
                held.touch = Some(pending);
            }
            InputEvent::Pen { ref mut event, .. } => {
                *event = touch_event::CANCEL_ALL;
                pending.delivery = Delivery::Reliable;
                held.pen = Some(pending);
            }
            // Motion, scrolling, text and the rest from the silence are dropped.
            _ => {}
        }
    }

    /// The number of packets held back since the host went silent.
    pub fn held_count(&self) -> u32 {
        self.held.dropped
    }

    /// Sends the latest held state: arrivals, then releases before presses, then controller
    /// state, then touch and pen cancels.
    fn release_held(&mut self, out: &mut Vec<InputPacket>) {
        let held = std::mem::take(&mut self.held);
        self.holding = false;
        let send = |pending: &Option<Pending>, out: &mut Vec<InputPacket>| {
            if let Some(p) = pending {
                out.push(InputPacket {
                    data: p.event.encode(self.sunshine),
                    channel: p.channel,
                    delivery: p.delivery,
                });
            }
        };
        let is_down = |p: &Option<Pending>| {
            matches!(
                p,
                Some(Pending {
                    event: InputEvent::Keyboard { down: true, .. }
                        | InputEvent::MouseButton { down: true, .. },
                    ..
                })
            )
        };
        for slot in &held.arrivals {
            send(slot, out);
        }
        for slot in held
            .keyboard
            .iter()
            .chain(&held.mouse_buttons)
            .filter(|p| !is_down(p))
        {
            send(slot, out);
        }
        for slot in held
            .keyboard
            .iter()
            .chain(&held.mouse_buttons)
            .filter(|p| is_down(p))
        {
            send(slot, out);
        }
        for number in 0..usize::from(MAX_CONTROLLERS) {
            send(&held.controllers[number], out);
            send(&held.motion[number][0], out);
            send(&held.motion[number][1], out);
            send(&held.batteries[number], out);
        }
        send(&held.touch, out);
        send(&held.pen, out);
    }
}

/// Splits UTF-8 into code points by their lead bytes, stopping at an invalid one as the C
/// does.
fn code_points(text: &[u8]) -> Vec<&[u8]> {
    let mut points = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let lead = text[i];
        let len = match lead {
            b if b & 0x80 == 0 => 1,
            b if b & 0xE0 == 0xC0 => 2,
            b if b & 0xF0 == 0xE0 => 3,
            b if b & 0xF8 == 0xF0 => 4,
            _ => break,
        };
        points.push(&text[i..(i + len).min(text.len())]);
        i += len;
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::negotiation::encryption;
    use butterpollo_core::input::{decode, Input};

    fn sunshine() -> HostCapabilities {
        HostCapabilities {
            app_version: [7, 1, 431, -1],
            feature_flags: feature::PEN_TOUCH_EVENTS | feature::CONTROLLER_TOUCH_EVENTS,
            encryption_enabled: encryption::CONTROL_V2,
            control_messages: vec![],
            client_roaming: false,
        }
    }

    fn gfe() -> HostCapabilities {
        HostCapabilities {
            app_version: [7, 1, 431, 0],
            feature_flags: 0,
            ..sunshine()
        }
    }

    fn queue(caps: &HostCapabilities) -> InputQueue {
        let mut q = InputQueue::new(caps);
        let mut out = Vec::new();
        q.poll(Instant::now(), false, &mut out);
        assert_eq!(decode(&out[0].data).unwrap(), Input::Haptics(true));
        q
    }

    fn take(q: &mut InputQueue, now: Instant) -> Vec<Input> {
        let mut out = Vec::new();
        q.poll(now, false, &mut out);
        out.iter().map(|p| decode(&p.data).unwrap()).collect()
    }

    #[test]
    fn touchpad_contacts_go_only_to_a_host_that_takes_them() {
        let contact = TouchpadContact { event: touch_event::MOVE, pointer_id: 1, x: 0.5, y: 0.5, pressure: 0.0 };
        let mut q = queue(&sunshine());
        assert_eq!(q.send_touchpad_frame(&[contact], 0, 0, 0), Err(InputError::Unsupported));
        assert_eq!(q.send_touchpad(contact, 0, 0, 0), Err(InputError::Unsupported));

        let caps = HostCapabilities {
            feature_flags: feature::TOUCHPAD_EVENTS | feature::TOUCHPAD_FRAME_EVENTS,
            ..sunshine()
        };
        let mut q = queue(&caps);
        assert_eq!(q.send_touchpad_frame(&[contact; 6], 0, 0, 0), Err(InputError::Invalid));
        // Moves drop for newer ones; the click and its release, or a new contact, never.
        q.send_touchpad_frame(&[contact; 2], 0, 105, 70).unwrap();
        q.send_touchpad_frame(&[contact], touchpad::BUTTON_PRIMARY, 105, 70).unwrap();
        q.send_touchpad_frame(&[contact], touchpad::BUTTON_PRIMARY, 105, 70).unwrap();
        q.send_touchpad(contact, 0, 105, 70).unwrap();
        q.send_touchpad(TouchpadContact { event: touch_event::DOWN, ..contact }, 0, 105, 70).unwrap();
        let mut out = Vec::new();
        q.poll(Instant::now(), false, &mut out);
        let sent: Vec<(u32, Delivery)> = out
            .iter()
            .map(|p| (u32::from_le_bytes(p.data[4..8].try_into().unwrap()), p.delivery))
            .collect();
        use super::super::packets::magic;
        assert_eq!(
            sent,
            [
                (magic::TOUCHPAD_FRAME, Delivery::Unreliable),
                (magic::TOUCHPAD_FRAME, Delivery::Reliable),
                (magic::TOUCHPAD_FRAME, Delivery::Unreliable),
                (magic::TOUCHPAD, Delivery::Reliable),
                (magic::TOUCHPAD, Delivery::Reliable),
            ]
        );
        assert!(out.iter().all(|p| p.channel == crate::control::wire::channel::TOUCH));
    }

    #[test]
    fn relative_motion_sums_and_splits() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        q.send_mouse_move(30_000, 1).unwrap();
        q.send_mouse_move(30_000, -1).unwrap();
        q.send_mouse_move(0, 0).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(
            take(&mut q, t0),
            [
                Input::Relative { x: 32767, y: 0 },
                Input::Relative { x: 27233, y: 0 }
            ]
        );
        // Within the batching interval the next motion waits and keeps gathering.
        q.send_mouse_move(5, 5).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            q.poll(t0, false, &mut out),
            Some(t0 + MOTION_BATCHING_INTERVAL)
        );
        assert!(out.is_empty());
        q.send_mouse_move(1, 1).unwrap();
        assert_eq!(
            take(&mut q, t0 + MOTION_BATCHING_INTERVAL),
            [Input::Relative { x: 6, y: 6 }]
        );
    }

    #[test]
    fn unbatched_motion_does_not_wait() {
        let mut q = queue(&sunshine());
        q.set_unbatched(true);
        let t0 = Instant::now();
        q.send_mouse_move(5, 5).unwrap();
        assert_eq!(take(&mut q, t0), [Input::Relative { x: 5, y: 5 }]);
        // The same instant: batched input would wait the interval, unbatched sends now.
        q.send_mouse_move(1, 1).unwrap();
        let mut out = Vec::new();
        assert_eq!(q.poll(t0, false, &mut out), None);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn batched_motion_still_waits_by_default() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        q.send_mouse_move(5, 5).unwrap();
        assert_eq!(take(&mut q, t0).len(), 1);
        q.send_mouse_move(1, 1).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            q.poll(t0, false, &mut out),
            Some(t0 + MOTION_BATCHING_INTERVAL)
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_disabled_queue_sends_nothing() {
        let mut q = InputQueue::new(&sunshine());
        assert_ne!(q.len(), 0, "the haptics enable is queued");
        q.disable();
        assert_eq!(q.len(), 0);
        assert!(q.send_mouse_move(1, 1).is_err());
        assert!(q.send_keyboard(0x41, true, 0, 0).is_err());
        let mut out = Vec::new();
        q.poll(Instant::now(), false, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn absolute_motion_sends_the_latest_position() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        q.send_mouse_position(10, 10, 1920, 1080).unwrap();
        q.send_mouse_position(20, 30, 1920, 1080).unwrap();
        assert_eq!(
            take(&mut q, t0),
            [Input::Absolute {
                x: 20,
                y: 30,
                width: 1919,
                height: 1079
            }]
        );
        q.send_mouse_move_as_position(5, -40, 1920, 1080).unwrap();
        let later = t0 + MOTION_BATCHING_INTERVAL;
        assert_eq!(
            take(&mut q, later),
            [Input::Absolute {
                x: 25,
                y: 0,
                width: 1919,
                height: 1079
            }]
        );
    }

    #[test]
    fn controllers_batch_until_buttons_change() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        q.send_controller(1, 3, 0, 0, 0, (100, 0), (0, 0)).unwrap();
        q.send_controller(1, 3, 0, 0, 0, (200, 0), (0, 0)).unwrap();
        q.send_controller(0, 3, button::A, 0, 0, (0, 0), (0, 0))
            .unwrap();
        q.send_controller(1, 3, 0, 9, 0, (300, 0), (0, 0)).unwrap();
        q.send_controller(1, 3, button::B, 0, 0, (400, 0), (0, 0))
            .unwrap();
        q.send_controller(1, 3, button::B, 0, 0, (500, 0), (0, 0))
            .unwrap();
        let sent = take(&mut q, t0);
        let sticks: Vec<(u16, i16)> = sent
            .iter()
            .map(|i| match i {
                Input::Controller { id, sticks, .. } => (*id, sticks[0]),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(sticks, [(1, 300), (0, 0), (1, 500)]);
        // A state already taken is not updated any more.
        q.send_controller(1, 3, button::B, 0, 0, (600, 0), (0, 0))
            .unwrap();
        assert_eq!(take(&mut q, t0).len(), 1);
    }

    #[test]
    fn gfe_rules() {
        let mut q = queue(&gfe());
        let t0 = Instant::now();
        q.send_controller(
            6,
            0xFF,
            button::MISC | button::PADDLE1,
            0,
            0,
            (0, 0),
            (0, 0),
        )
        .unwrap();
        q.send_keyboard(0xA1, true, super::super::packets::modifier::SHIFT | 0x10, 7)
            .unwrap();
        q.send_keyboard(0x5B, true, super::super::packets::modifier::META, 0)
            .unwrap();
        assert_eq!(q.send_high_res_hscroll(10), Err(InputError::Unsupported));
        assert_eq!(
            q.send_touch(1, 1, 0., 0., 0., 0., 0., 0),
            Err(InputError::Unsupported)
        );
        assert_eq!(
            q.send_controller_battery(0, 1, 1),
            Err(InputError::Unsupported)
        );
        q.send_high_res_scroll(60).unwrap();
        q.send_high_res_scroll(90).unwrap();
        q.send_high_res_scroll(-10).unwrap();
        let sent = take(&mut q, t0);
        assert_eq!(
            sent[0],
            Input::Controller {
                id: 2,
                active: 0xF,
                buttons: button::SPECIAL,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4]
            }
        );
        assert_eq!(
            sent[1],
            Input::Keyboard {
                key: 0xA1,
                modifiers: 0,
                flags: 0,
                down: true
            }
        );
        assert_eq!(
            sent[2],
            Input::Keyboard {
                key: 0x5B,
                modifiers: 0,
                flags: 0,
                down: true
            }
        );
        assert_eq!(
            sent[3],
            Input::Scroll {
                amount: 120,
                horizontal: false
            }
        );
        assert_eq!(sent.len(), 4);
    }

    #[test]
    fn arrival_sends_a_neutral_state_too() {
        let mut q = queue(&sunshine());
        q.send_controller_arrival(17, 2, 2, 0xFFFF, capability::DUAL_TOUCHPAD)
            .unwrap();
        let sent = take(&mut q, Instant::now());
        assert_eq!(
            sent[0],
            Input::Arrival {
                id: 1,
                kind: 2,
                capabilities: 0x108,
                buttons: 0xFFFF
            }
        );
        assert!(matches!(
            sent[1],
            Input::Controller {
                id: 1,
                active: 2,
                buttons: 0,
                ..
            }
        ));
        let mut g = queue(&gfe());
        g.send_controller_arrival(0, 1, 1, 0, 0).unwrap();
        assert_eq!(take(&mut g, Instant::now()).len(), 1);
    }

    #[test]
    fn motion_keeps_the_latest_reading_and_parks_the_gyro_reliably() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        assert_eq!(
            q.send_controller_motion(0, 3, 0., 0., 0.),
            Err(InputError::Invalid)
        );
        assert_eq!(
            q.send_controller_motion(0, 0, 0., 0., 0.),
            Err(InputError::Invalid)
        );
        q.send_controller_motion(0, motion_type::ACCEL, 1., 2., 3.)
            .unwrap();
        q.send_controller_motion(0, motion_type::ACCEL, 4., 5., 6.)
            .unwrap();
        q.send_controller_motion(0, motion_type::GYRO, 0., 0., 0.)
            .unwrap();
        let mut out = Vec::new();
        q.poll(t0, false, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(
            decode(&out[0].data).unwrap(),
            Input::Motion {
                id: 0,
                kind: 1,
                xyz: [4., 5., 6.]
            }
        );
        assert_eq!(out[0].delivery, Delivery::Unreliable);
        assert_eq!(out[0].channel, 0x20);
        assert_eq!(out[1].delivery, Delivery::Reliable);
    }

    /// A pen's roll goes as 0x5500000B only to a host that takes it; otherwise, and for a
    /// roll out of range, the pen packet stays the stock one.
    #[test]
    fn the_barrel_roll_goes_only_where_it_is_offered() {
        let magics = |offered: bool, roll: u16| {
            let mut q = queue(&sunshine());
            q.set_pen_barrel_roll(offered);
            q.send_pen(touch_event::DOWN, 1, 0, 0.5, 0.5, 0.5, 0., 0., 0, 0, roll).unwrap();
            let mut out = Vec::new();
            q.poll(Instant::now(), false, &mut out);
            assert_eq!(out.len(), 1);
            (u32::from_le_bytes(out[0].data[4..8].try_into().unwrap()), out[0].data.len())
        };
        assert_eq!(magics(false, 90), (0x5500_0003, 36));
        assert_eq!(magics(true, pen::ROLL_UNKNOWN), (0x5500_0003, 36));
        assert_eq!(magics(true, 360), (0x5500_0003, 36));
        assert_eq!(magics(true, 90), (pen::PEN_BARREL_ROLL_MAGIC, 40));
    }

    #[test]
    fn pen_moves_collapse_and_button_changes_go_reliably() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        let pen = |q: &mut InputQueue, event, buttons, x| {
            q.send_pen(event, 1, buttons, x, 0.5, 0.5, 0., 0., 0, 0, pen::ROLL_UNKNOWN)
        };
        pen(&mut q, touch_event::DOWN, 0, 0.1).unwrap();
        pen(&mut q, touch_event::MOVE, 0, 0.2).unwrap();
        pen(&mut q, touch_event::MOVE, 0, 0.3).unwrap();
        pen(&mut q, touch_event::MOVE, 1, 0.4).unwrap();
        let mut out = Vec::new();
        q.poll(t0, false, &mut out);
        let xs: Vec<(f32, Delivery)> = out
            .iter()
            .map(|p| match decode(&p.data).unwrap() {
                Input::Pen { x, .. } => (x, p.delivery),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(xs, [(0.1, Delivery::Reliable), (0.3, Delivery::Unreliable)]);
        let mut out = Vec::new();
        assert_eq!(
            q.poll(t0, false, &mut out),
            Some(t0 + MOTION_BATCHING_INTERVAL)
        );
        q.poll(t0 + MOTION_BATCHING_INTERVAL, false, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].delivery, Delivery::Reliable);
    }

    #[test]
    fn text_waits_then_goes_one_code_point_per_packet() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        q.send_keyboard(0x41, true, 0, 0).unwrap();
        q.send_text("aé€😀").unwrap();
        let mut out = Vec::new();
        assert_eq!(q.poll(t0, false, &mut out), Some(t0 + TEXT_SETTLE));
        assert_eq!(out.len(), 1);
        let sent = take(&mut q, t0 + TEXT_SETTLE);
        assert_eq!(
            sent,
            ["a", "é", "€", "😀"]
                .map(|s| Input::Text(s.into()))
                .to_vec()
        );
        assert_eq!(code_points(&[b'a', 0xFF, b'b']), vec![&b"a"[..]]);
    }

    #[test]
    fn silent_host_input_is_held_then_replaced_by_the_current_state() {
        let mut q = queue(&sunshine());
        let t0 = Instant::now();
        let mut out = Vec::new();
        q.send_keyboard(0x41, true, 0, 0).unwrap();
        q.send_keyboard(0x42, true, 0, 0).unwrap();
        q.send_keyboard(0x41, false, 0, 0).unwrap();
        q.send_mouse_button(1, true).unwrap();
        q.send_mouse_move(10, 10).unwrap();
        q.send_touch(touch_event::DOWN, 1, 0.5, 0.5, 1., 0., 0., 0)
            .unwrap();
        q.send_controller(0, 1, button::A, 0, 0, (5, 5), (0, 0))
            .unwrap();
        q.send_text("x").unwrap();
        q.poll(t0, true, &mut out);
        assert!(out.is_empty());
        assert!(q.is_holding());
        assert!(q.held_count() >= 7);

        q.poll(t0, false, &mut out);
        let sent: Vec<Input> = out.iter().map(|p| decode(&p.data).unwrap()).collect();
        assert_eq!(
            sent,
            [
                Input::Keyboard {
                    key: 0x41,
                    modifiers: 0,
                    flags: 0,
                    down: false
                },
                Input::Keyboard {
                    key: 0x42,
                    modifiers: 0,
                    flags: 0,
                    down: true
                },
                Input::MouseButton {
                    button: 1,
                    down: true
                },
                Input::Controller {
                    id: 0,
                    active: 1,
                    buttons: button::A,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [5, 5, 0, 0]
                },
                Input::Touch {
                    event: 7,
                    id: 1,
                    x: 0.5,
                    y: 0.5,
                    pressure: 1.,
                    major: 0.,
                    minor: 0.,
                    rotation: 0
                },
            ]
        );
        assert_eq!(out.last().unwrap().delivery, Delivery::Reliable);
        assert!(!q.is_holding());
    }

    #[test]
    fn stopped_and_full_queues_refuse_input() {
        let mut q = queue(&sunshine());
        for i in 0..MAX_QUEUED {
            q.send_mouse_button(1, i % 2 == 0).unwrap();
        }
        assert_eq!(q.send_mouse_button(1, true), Err(InputError::QueueFull));
        let mut out = Vec::new();
        q.drain(Instant::now(), false, &mut out);
        assert_eq!(out.len(), MAX_QUEUED);
        q.stop();
        assert_eq!(q.send_keyboard(1, true, 0, 0), Err(InputError::Stopped));
    }
}
