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
/// A relative motion floor that a 1 kHz mouse's reports, jitter included, stay clear of, while
/// a 4 or 8 kHz mouse is merged ([`InputQueue::set_relative_motion_floor`]).
pub const FAST_MOUSE_FLOOR: Duration = Duration::from_micros(500);
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
    identities: [Option<Pending>; MAX_CONTROLLERS as usize],
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
            identities: Default::default(),
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

    /// Sends controller identities (0x55000080) and the C button: only for a host with
    /// `ControllerIdentityVersion` (`Feature::ControllerIdentity`). Otherwise identities are
    /// dropped and the C button is cleared.
    pub fn set_controller_identity(&mut self, offered: bool) {
        self.controller_identity = offered;
    }

    /// Whether [`Self::send_controller_identity`] and the C button reach the host.
    pub fn controller_identity(&self) -> bool {
        self.controller_identity
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
        // Only a host that takes controller identities knows the C button.
        if !self.controller_identity {
            buttons &= !button::C;
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

    /// Tells a host with `ControllerIdentityVersion` which controller `number` is, by its USB
    /// vendor and product. Send it before the arrival: the host chooses the virtual pad then.
    /// Nothing is sent to any other host.
    pub fn send_controller_identity(
        &mut self,
        number: u8,
        vendor: u16,
        product: u16,
    ) -> Result<(), InputError> {
        self.check()?;
        if !self.controller_identity {
            return Ok(());
        }
        self.offer_event(InputEvent::ControllerIdentity {
            number: number % MAX_CONTROLLERS,
            vendor,
            product,
        })
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
        }))?;
        // Only once it is queued: a refused event leaves the buttons the host last got,
        // so the click tried again is still a change.
        self.pen_buttons = buttons;
        Ok(())
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
                Entry::RelativeMouse => {
                    self.last_mouse.map(|t| t + motion_interval.max(self.relative_floor))
                }
                Entry::AbsoluteMouse => self.last_mouse.map(|t| t + motion_interval),
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
            // A button change travels reliably; the newer move that carries the same
            // buttons takes that over with its place.
            let reliable = matches!(pending.delivery, Delivery::Reliable);
            pending = next;
            if reliable {
                pending.delivery = Delivery::Reliable;
            }
        }
        pending
    }

    fn emit(&mut self, pending: Pending, contact_lost: bool, out: &mut Vec<InputPacket>) {
        if contact_lost {
            self.hold(pending);
            return;
        }
        note_sent(self.frame_warp.as_deref(), &pending.event);
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
            InputEvent::ControllerIdentity { number, .. } => {
                if let Some(slot) = held.identities.get_mut(usize::from(number)) {
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

    /// Sends the latest held state: identities and arrivals, then releases before presses, then controller
    /// state, then touch and pen cancels.
    fn release_held(&mut self, out: &mut Vec<InputPacket>) {
        let held = std::mem::take(&mut self.held);
        self.holding = false;
        let frame_warp = self.frame_warp.clone();
        let send = |pending: &Option<Pending>, out: &mut Vec<InputPacket>| {
            if let Some(p) = pending {
                note_sent(frame_warp.as_deref(), &p.event);
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
        // Each identity goes before the arrivals: the host picks the pad on arrival.
        for slot in held.identities.iter().chain(&held.arrivals) {
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

/// Counts a packet that goes on the wire for Frame Warp: relative mouse motion and controller
/// state, exactly as sent.
fn note_sent(frame_warp: Option<&crate::frame_warp::FrameWarp>, event: &InputEvent) {
    let Some(frame_warp) = frame_warp else { return };
    match *event {
        InputEvent::MouseMove { dx, dy } => frame_warp.note_mouse(dx, dy),
        InputEvent::Controller {
            number,
            right_stick,
            ..
        } => frame_warp.note_controller(number, right_stick),
        _ => {}
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
mod tests;
