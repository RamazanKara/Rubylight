//! Input packet layouts (`Input.h`). Every packet starts with its length after the length
//! field (big-endian) and a little-endian magic; the fields that follow are little-endian
//! except mouse motion and scroll, which are big-endian.

use crate::control::wire::{channel, Delivery};

pub mod magic {
    pub const ENABLE_HAPTICS: u32 = 0x0000_000D;
    pub const KEY_DOWN: u32 = 0x0000_0003;
    pub const KEY_UP: u32 = 0x0000_0004;
    pub const UTF8_TEXT: u32 = 0x0000_0017;
    pub const MOUSE_MOVE_REL: u32 = 0x0000_0007;
    pub const MOUSE_MOVE_ABS: u32 = 0x0000_0005;
    pub const MOUSE_BUTTON_DOWN: u32 = 0x0000_0008;
    pub const MOUSE_BUTTON_UP: u32 = 0x0000_0009;
    pub const MULTI_CONTROLLER: u32 = 0x0000_000C;
    pub const SCROLL: u32 = 0x0000_000A;
    pub const HSCROLL: u32 = 0x5500_0001;
    pub const TOUCH: u32 = 0x5500_0002;
    pub const PEN: u32 = 0x5500_0003;
    /// The pen packet with the barrel roll after it (`rubylight_protocol::pen`).
    pub const PEN_BARREL_ROLL: u32 = rubylight_protocol::pen::PEN_BARREL_ROLL_MAGIC;
    pub const CONTROLLER_ARRIVAL: u32 = 0x5500_0004;
    pub const CONTROLLER_TOUCH: u32 = 0x5500_0005;
    pub const CONTROLLER_MOTION: u32 = 0x5500_0006;
    pub const CONTROLLER_BATTERY: u32 = 0x5500_0007;
    /// Foundation Sunshine's laptop touchpad packets (Moonlight V+'s moonlight-common-c).
    pub const TOUCHPAD: u32 = 0x5500_0009;
    pub const TOUCHPAD_FRAME: u32 = 0x5500_000A;
    /// Which controller a client has (`rubylight_protocol::controller_identity`).
    pub const CONTROLLER_IDENTITY: u32 =
        rubylight_protocol::controller_identity::CONTROLLER_IDENTITY_MAGIC;
}

/// Laptop touchpad packets (`SS_TOUCHPAD_*`): x and y are on the touchpad's own surface, 0-1.
pub mod touchpad {
    /// `LI_TOUCHPAD_BUTTON_PRIMARY`: the touchpad's click is down.
    pub const BUTTON_PRIMARY: u8 = 0x01;
    /// Contacts in one frame packet.
    pub const MAX_FRAME_CONTACTS: usize = 5;
    /// `LI_ROT_UNKNOWN`.
    pub const ROTATION_UNKNOWN: u16 = 0xFFFF;
}

/// One contact of a touchpad frame packet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchpadContact {
    pub event: u8,
    pub pointer_id: u32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// moonlight-common-c's `normalizedFloatToUint16`.
fn unit_u16(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
}

/// `LI_TOUCH_EVENT_*`, for touch, pen and controller touch events.
pub mod touch_event {
    pub const HOVER: u8 = 0x00;
    pub const DOWN: u8 = 0x01;
    pub const UP: u8 = 0x02;
    pub const MOVE: u8 = 0x03;
    pub const CANCEL: u8 = 0x04;
    pub const BUTTON_ONLY: u8 = 0x05;
    pub const HOVER_LEAVE: u8 = 0x06;
    pub const CANCEL_ALL: u8 = 0x07;

    /// Hover and move events may be dropped for a newer one.
    pub fn is_batchable(event: u8) -> bool {
        event == HOVER || event == MOVE
    }
}

/// `LI_MOTION_TYPE_*`.
pub mod motion_type {
    pub const ACCEL: u8 = 0x01;
    pub const GYRO: u8 = 0x02;
}

/// `MODIFIER_*` for keyboard events.
pub mod modifier {
    pub const SHIFT: u8 = 0x01;
    pub const CTRL: u8 = 0x02;
    pub const ALT: u8 = 0x04;
    pub const META: u8 = 0x08;
    /// An extended key (0xE0 scancode prefix); a Sunshine extension.
    pub const EXTENDED: u8 = 0x10;
}

/// `BUTTON_*` mouse buttons.
pub mod mouse_button {
    pub const LEFT: u8 = 0x01;
    pub const MIDDLE: u8 = 0x02;
    pub const RIGHT: u8 = 0x03;
    pub const X1: u8 = 0x04;
    pub const X2: u8 = 0x05;
}

/// Controller button flags (`*_FLAG`); the upper 16 bits are Sunshine extensions.
pub mod button {
    pub const UP: u32 = 0x0001;
    pub const DOWN: u32 = 0x0002;
    pub const LEFT: u32 = 0x0004;
    pub const RIGHT: u32 = 0x0008;
    pub const PLAY: u32 = 0x0010;
    pub const BACK: u32 = 0x0020;
    pub const LS_CLK: u32 = 0x0040;
    pub const RS_CLK: u32 = 0x0080;
    pub const LB: u32 = 0x0100;
    pub const RB: u32 = 0x0200;
    pub const SPECIAL: u32 = 0x0400;
    pub const A: u32 = 0x1000;
    pub const B: u32 = 0x2000;
    pub const X: u32 = 0x4000;
    pub const Y: u32 = 0x8000;
    pub const PADDLE1: u32 = 0x01_0000;
    pub const PADDLE2: u32 = 0x02_0000;
    pub const PADDLE3: u32 = 0x04_0000;
    pub const PADDLE4: u32 = 0x08_0000;
    pub const TOUCHPAD: u32 = 0x10_0000;
    pub const MISC: u32 = 0x20_0000;
    /// The Switch 2 Pro Controller's C button. Only for a host with
    /// `ControllerIdentityVersion`: any other drops the packet's upper bits or misreads them.
    pub const C: u32 = rubylight_protocol::controller_identity::BUTTON_C;
}

/// Controller capabilities (`LI_CCAP_*`) of an arrival event.
pub mod capability {
    pub const ANALOG_TRIGGERS: u16 = 0x01;
    pub const RUMBLE: u16 = 0x02;
    pub const TRIGGER_RUMBLE: u16 = 0x04;
    pub const TOUCHPAD: u16 = 0x08;
    pub const ACCEL: u16 = 0x10;
    pub const GYRO: u16 = 0x20;
    pub const BATTERY_STATE: u16 = 0x40;
    pub const RGB_LED: u16 = 0x80;
    pub const DUAL_TOUCHPAD: u16 = 0x100;
    pub const HAPTICS_PCM: u16 = crate::control::haptics::CAPABILITY;
}

/// Most controllers (bits in the active gamepad mask).
pub const MAX_CONTROLLERS: u8 = 16;

/// One input event, as encoded on the wire.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// Tells GFE to send rumble; sent once when input starts.
    EnableHaptics,
    Keyboard {
        key_code: u16,
        down: bool,
        modifiers: u8,
        flags: u8,
    },
    /// UTF-8 text; the queue sends one code point per packet.
    Text(Vec<u8>),
    MouseMove {
        dx: i16,
        dy: i16,
    },
    /// `width` and `height` are as sent: the queue subtracts one from the reference size.
    MousePosition {
        x: i16,
        y: i16,
        width: i16,
        height: i16,
    },
    MouseButton {
        button: u8,
        down: bool,
    },
    Scroll {
        amount: i16,
    },
    HScroll {
        amount: i16,
    },
    Controller {
        number: u16,
        active_mask: u16,
        buttons: u32,
        left_trigger: u8,
        right_trigger: u8,
        left_stick: (i16, i16),
        right_stick: (i16, i16),
    },
    ControllerArrival {
        number: u8,
        kind: u8,
        capabilities: u16,
        supported_buttons: u32,
    },
    /// The controller's USB vendor and product, sent before its arrival.
    ControllerIdentity {
        number: u8,
        vendor: u16,
        product: u16,
    },
    Touch {
        event: u8,
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure_or_distance: f32,
        contact_major: f32,
        contact_minor: f32,
        rotation: u16,
    },
    Pen {
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
        /// Barrel roll, degrees 0-359 clockwise; a known one makes this 0x5500000B, and
        /// `rubylight_protocol::pen::ROLL_UNKNOWN` keeps the stock 0x55000003.
        roll: u16,
    },
    ControllerTouch {
        number: u8,
        event: u8,
        touchpad: u8,
        pointer_id: u32,
        x: f32,
        y: f32,
        pressure: f32,
    },
    ControllerMotion {
        number: u8,
        motion_type: u8,
        x: f32,
        y: f32,
        z: f32,
    },
    ControllerBattery {
        number: u8,
        state: u8,
        percentage: u8,
    },
    /// One laptop touchpad contact (`LiSendTouchpadEvent`).
    Touchpad {
        event: u8,
        buttons: u8,
        rotation: u16,
        pointer_id: u32,
        width_mm: u16,
        height_mm: u16,
        x: f32,
        y: f32,
        pressure: f32,
        contact_major: f32,
        contact_minor: f32,
    },
    /// The contacts of one laptop touchpad frame (`LiSendTouchpadFrameEvent`); `count` of
    /// `contacts` are used, the rest go as zeros.
    TouchpadFrame {
        count: u8,
        buttons: u8,
        rotation: u16,
        width_mm: u16,
        height_mm: u16,
        contacts: [TouchpadContact; touchpad::MAX_FRAME_CONTACTS],
    },
}

impl InputEvent {
    pub fn magic(&self) -> u32 {
        match self {
            Self::EnableHaptics => magic::ENABLE_HAPTICS,
            Self::Keyboard { down: true, .. } => magic::KEY_DOWN,
            Self::Keyboard { down: false, .. } => magic::KEY_UP,
            Self::Text(_) => magic::UTF8_TEXT,
            Self::MouseMove { .. } => magic::MOUSE_MOVE_REL,
            Self::MousePosition { .. } => magic::MOUSE_MOVE_ABS,
            Self::MouseButton { down: true, .. } => magic::MOUSE_BUTTON_DOWN,
            Self::MouseButton { down: false, .. } => magic::MOUSE_BUTTON_UP,
            Self::Scroll { .. } => magic::SCROLL,
            Self::HScroll { .. } => magic::HSCROLL,
            Self::Controller { .. } => magic::MULTI_CONTROLLER,
            Self::ControllerArrival { .. } => magic::CONTROLLER_ARRIVAL,
            Self::ControllerIdentity { .. } => magic::CONTROLLER_IDENTITY,
            Self::Touch { .. } => magic::TOUCH,
            Self::Pen {
                roll: rubylight_protocol::pen::ROLL_UNKNOWN,
                ..
            } => magic::PEN,
            Self::Pen { .. } => magic::PEN_BARREL_ROLL,
            Self::ControllerTouch { .. } => magic::CONTROLLER_TOUCH,
            Self::ControllerMotion { .. } => magic::CONTROLLER_MOTION,
            Self::ControllerBattery { .. } => magic::CONTROLLER_BATTERY,
            Self::Touchpad { .. } => magic::TOUCHPAD,
            Self::TouchpadFrame { .. } => magic::TOUCHPAD_FRAME,
        }
    }

    /// The packet as the host parses it. `sunshine` carries the upper controller button
    /// flags, which GFE does not take.
    pub fn encode(&self, sunshine: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(40);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&self.magic().to_le_bytes());
        let le16 = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_le_bytes());
        let be16 = |out: &mut Vec<u8>, v: i16| out.extend_from_slice(&v.to_be_bytes());
        let float = |out: &mut Vec<u8>, v: f32| out.extend_from_slice(&v.to_le_bytes());
        match *self {
            Self::EnableHaptics => le16(&mut out, 1),
            Self::Keyboard {
                key_code,
                modifiers,
                flags,
                ..
            } => {
                out.push(flags);
                le16(&mut out, key_code);
                out.push(modifiers);
                le16(&mut out, 0);
            }
            Self::Text(ref text) => out.extend_from_slice(text),
            Self::MouseMove { dx, dy } => {
                be16(&mut out, dx);
                be16(&mut out, dy);
            }
            Self::MousePosition {
                x,
                y,
                width,
                height,
            } => {
                be16(&mut out, x);
                be16(&mut out, y);
                be16(&mut out, 0);
                be16(&mut out, width);
                be16(&mut out, height);
            }
            Self::MouseButton { button, .. } => out.push(button),
            Self::Scroll { amount } => {
                be16(&mut out, amount);
                be16(&mut out, amount);
                be16(&mut out, 0);
            }
            Self::HScroll { amount } => be16(&mut out, amount),
            Self::Controller {
                number,
                active_mask,
                buttons,
                left_trigger,
                right_trigger,
                left_stick,
                right_stick,
            } => {
                le16(&mut out, 0x001A);
                le16(&mut out, number);
                le16(&mut out, active_mask);
                le16(&mut out, 0x0014);
                le16(&mut out, buttons as u16);
                out.push(left_trigger);
                out.push(right_trigger);
                for axis in [left_stick.0, left_stick.1, right_stick.0, right_stick.1] {
                    le16(&mut out, axis as u16);
                }
                le16(&mut out, 0x009C);
                le16(&mut out, if sunshine { (buttons >> 16) as u16 } else { 0 });
                le16(&mut out, 0x0055);
            }
            Self::ControllerArrival {
                number,
                kind,
                capabilities,
                supported_buttons,
            } => {
                out.push(number);
                out.push(kind);
                le16(&mut out, capabilities);
                out.extend_from_slice(&supported_buttons.to_le_bytes());
            }
            Self::ControllerIdentity {
                number,
                vendor,
                product,
            } => {
                // `ControllerIdentity::encode`: controller, reserved, vendor, product, reserved.
                out.extend_from_slice(&[number, 0]);
                le16(&mut out, vendor);
                le16(&mut out, product);
                out.extend_from_slice(&[0, 0]);
            }
            Self::Touch {
                event,
                pointer_id,
                x,
                y,
                pressure_or_distance,
                contact_major,
                contact_minor,
                rotation,
            } => {
                out.push(event);
                out.push(0);
                le16(&mut out, rotation);
                out.extend_from_slice(&pointer_id.to_le_bytes());
                for v in [x, y, pressure_or_distance, contact_major, contact_minor] {
                    float(&mut out, v);
                }
            }
            Self::Pen {
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
                roll,
            } => {
                out.extend_from_slice(&[event, tool, buttons, 0]);
                for v in [x, y, pressure_or_distance] {
                    float(&mut out, v);
                }
                le16(&mut out, rotation);
                out.extend_from_slice(&[tilt, 0]);
                float(&mut out, contact_major);
                float(&mut out, contact_minor);
                if roll != rubylight_protocol::pen::ROLL_UNKNOWN {
                    le16(&mut out, roll);
                    le16(&mut out, 0);
                }
            }
            Self::ControllerTouch {
                number,
                event,
                touchpad,
                pointer_id,
                x,
                y,
                pressure,
            } => {
                out.extend_from_slice(&[number, event, 0, touchpad]);
                out.extend_from_slice(&pointer_id.to_le_bytes());
                for v in [x, y, pressure] {
                    float(&mut out, v);
                }
            }
            Self::ControllerMotion {
                number,
                motion_type,
                x,
                y,
                z,
            } => {
                out.extend_from_slice(&[number, motion_type, 0, 0]);
                for v in [x, y, z] {
                    float(&mut out, v);
                }
            }
            Self::ControllerBattery {
                number,
                state,
                percentage,
            } => {
                out.extend_from_slice(&[number, state, percentage, 0]);
            }
            Self::Touchpad {
                event,
                buttons,
                rotation,
                pointer_id,
                width_mm,
                height_mm,
                x,
                y,
                pressure,
                contact_major,
                contact_minor,
            } => {
                out.extend_from_slice(&[event, buttons]);
                le16(&mut out, rotation);
                out.extend_from_slice(&pointer_id.to_le_bytes());
                le16(&mut out, width_mm);
                le16(&mut out, height_mm);
                for v in [x, y, pressure, contact_major, contact_minor] {
                    float(&mut out, v);
                }
            }
            Self::TouchpadFrame {
                count,
                buttons,
                rotation,
                width_mm,
                height_mm,
                contacts,
            } => {
                out.extend_from_slice(&[count, buttons]);
                le16(&mut out, rotation);
                le16(&mut out, width_mm);
                le16(&mut out, height_mm);
                for (i, c) in contacts.iter().enumerate() {
                    let c = if i < usize::from(count) { *c } else { TouchpadContact::default() };
                    out.extend_from_slice(&[c.event, 0]);
                    le16(&mut out, unit_u16(c.pressure));
                    out.extend_from_slice(&c.pointer_id.to_le_bytes());
                    le16(&mut out, unit_u16(c.x));
                    le16(&mut out, unit_u16(c.y));
                }
            }
        }
        let size = (out.len() - 4) as u32;
        out[..4].copy_from_slice(&size.to_be_bytes());
        out
    }

    /// The ENet channel the C sends it on.
    pub fn channel(&self) -> u8 {
        match *self {
            Self::EnableHaptics => channel::GENERIC,
            Self::Keyboard { .. } => channel::KEYBOARD,
            Self::Text(_) => channel::UTF8,
            Self::MouseMove { .. }
            | Self::MousePosition { .. }
            | Self::MouseButton { .. }
            | Self::Scroll { .. }
            | Self::HScroll { .. } => channel::MOUSE,
            Self::Touch { .. } | Self::Touchpad { .. } | Self::TouchpadFrame { .. } => {
                channel::TOUCH
            }
            Self::Pen { .. } => channel::PEN,
            Self::Controller { number, .. } => {
                channel::GAMEPAD_BASE + (number % u16::from(MAX_CONTROLLERS)) as u8
            }
            Self::ControllerArrival { number, .. }
            | Self::ControllerIdentity { number, .. }
            | Self::ControllerTouch { number, .. }
            | Self::ControllerBattery { number, .. } => {
                channel::GAMEPAD_BASE + number % MAX_CONTROLLERS
            }
            Self::ControllerMotion { number, .. } => {
                channel::SENSOR_BASE + number % MAX_CONTROLLERS
            }
        }
    }

    /// The delivery the C uses, before the queue's pen and motion rules: touch and
    /// controller touch moves and hovers may be dropped, everything else is reliable.
    pub fn default_delivery(&self) -> Delivery {
        match *self {
            Self::Touch { event, .. } | Self::ControllerTouch { event, .. }
                if touch_event::is_batchable(event) =>
            {
                Delivery::Unreliable
            }
            _ => Delivery::Reliable,
        }
    }
}

#[cfg(test)]
mod tests;
