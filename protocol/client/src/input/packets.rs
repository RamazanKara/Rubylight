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
mod tests {
    use super::*;
    use butterpollo_core::input::{decode, Input};

    fn host(event: &InputEvent) -> Input {
        decode(&event.encode(true)).unwrap_or_else(|e| panic!("{event:?}: {e}"))
    }

    #[test]
    fn packets_decode_on_the_host() {
        assert_eq!(host(&InputEvent::EnableHaptics), Input::Haptics(true));
        assert_eq!(
            host(&InputEvent::Keyboard {
                key_code: 0x8041,
                down: true,
                modifiers: modifier::SHIFT,
                flags: 1
            }),
            Input::Keyboard {
                key: 0x41,
                modifiers: 1,
                flags: 1,
                down: true
            }
        );
        assert_eq!(
            host(&InputEvent::Keyboard {
                key_code: 0x80A5,
                down: false,
                modifiers: modifier::EXTENDED,
                flags: 0
            }),
            Input::Keyboard {
                key: 0xA5,
                modifiers: 0x10,
                flags: 0,
                down: false
            }
        );
        assert_eq!(
            host(&InputEvent::Text("é".as_bytes().to_vec())),
            Input::Text("é".into())
        );
        assert_eq!(
            host(&InputEvent::MouseMove { dx: -300, dy: 7 }),
            Input::Relative { x: -300, y: 7 }
        );
        assert_eq!(
            host(&InputEvent::MousePosition {
                x: 100,
                y: 200,
                width: 1919,
                height: 1079
            }),
            Input::Absolute {
                x: 100,
                y: 200,
                width: 1919,
                height: 1079
            }
        );
        assert_eq!(
            host(&InputEvent::MouseButton {
                button: mouse_button::X2,
                down: true
            }),
            Input::MouseButton {
                button: 5,
                down: true
            }
        );
        assert_eq!(
            host(&InputEvent::MouseButton {
                button: mouse_button::LEFT,
                down: false
            }),
            Input::MouseButton {
                button: 1,
                down: false
            }
        );
        assert_eq!(
            host(&InputEvent::Scroll { amount: -120 }),
            Input::Scroll {
                amount: -120,
                horizontal: false
            }
        );
        assert_eq!(
            host(&InputEvent::HScroll { amount: 60 }),
            Input::Scroll {
                amount: 60,
                horizontal: true
            }
        );
        assert_eq!(
            host(&InputEvent::Controller {
                number: 3,
                active_mask: 0b1001,
                buttons: button::A | button::PADDLE2 | button::MISC,
                left_trigger: 10,
                right_trigger: 255,
                left_stick: (-32768, 32767),
                right_stick: (1, -1),
            }),
            Input::Controller {
                id: 3,
                active: 0b1001,
                buttons: button::A | button::PADDLE2 | button::MISC,
                left_trigger: 10,
                right_trigger: 255,
                sticks: [-32768, 32767, 1, -1],
            }
        );
        assert_eq!(
            host(&InputEvent::ControllerArrival {
                number: 2,
                kind: 2,
                capabilities: 0x8109,
                supported_buttons: 0x3f_ffff
            }),
            Input::Arrival {
                id: 2,
                kind: 2,
                capabilities: 0x8109,
                buttons: 0x3f_ffff
            }
        );
        assert_eq!(
            host(&InputEvent::Touch {
                event: touch_event::MOVE,
                pointer_id: 9,
                x: 0.25,
                y: 0.5,
                pressure_or_distance: 0.75,
                contact_major: 0.1,
                contact_minor: 0.05,
                rotation: 90,
            }),
            Input::Touch {
                event: 3,
                id: 9,
                x: 0.25,
                y: 0.5,
                pressure: 0.75,
                major: 0.1,
                minor: 0.05,
                rotation: 90
            }
        );
        assert_eq!(
            host(&InputEvent::Pen {
                event: touch_event::DOWN,
                tool: 2,
                buttons: 1,
                x: 0.1,
                y: 0.2,
                pressure_or_distance: 0.3,
                contact_major: 0.0,
                contact_minor: 0.0,
                rotation: 45,
                tilt: 30,
                roll: rubylight_protocol::pen::ROLL_UNKNOWN,
            }),
            Input::Pen {
                event: 1,
                tool: 2,
                buttons: 1,
                x: 0.1,
                y: 0.2,
                pressure: 0.3,
                rotation: 45,
                tilt: 30
            }
        );
        assert_eq!(
            host(&InputEvent::ControllerTouch {
                number: 1,
                event: 1,
                touchpad: 1,
                pointer_id: 4,
                x: 0.5,
                y: 0.5,
                pressure: 1.0
            }),
            Input::ControllerTouch {
                id: 1,
                event: 1,
                touchpad: 1,
                pointer: 4,
                x: 0.5,
                y: 0.5,
                pressure: 1.0
            }
        );
        assert_eq!(
            host(&InputEvent::ControllerMotion {
                number: 0,
                motion_type: motion_type::GYRO,
                x: 1.0,
                y: -2.0,
                z: 3.5
            }),
            Input::Motion {
                id: 0,
                kind: 2,
                xyz: [1.0, -2.0, 3.5]
            }
        );
        assert_eq!(
            host(&InputEvent::ControllerBattery {
                number: 15,
                state: 3,
                percentage: 80
            }),
            Input::Battery {
                id: 15,
                state: 3,
                percent: 80
            }
        );
    }

    /// A known roll makes the pen packet 0x5500000B, the stock packet plus the roll; an
    /// unknown roll leaves it byte for byte the stock 0x55000003.
    #[test]
    fn a_known_barrel_roll_extends_the_pen_packet() {
        use rubylight_protocol::pen::{PenBarrelRoll, ROLL_UNKNOWN};
        let pen = |roll| InputEvent::Pen {
            event: touch_event::MOVE,
            tool: 1,
            buttons: 0,
            x: 0.25,
            y: 0.75,
            pressure_or_distance: 0.5,
            contact_major: 0.0,
            contact_minor: 0.0,
            rotation: 90,
            tilt: 30,
            roll,
        };
        let stock = pen(ROLL_UNKNOWN).encode(true);
        assert_eq!(stock.len(), 36);
        assert_eq!(stock[4..8], magic::PEN.to_le_bytes());
        for roll in [0, 270, 359] {
            let wire = pen(roll).encode(true);
            let decoded = PenBarrelRoll::decode(&wire).unwrap_or_else(|| panic!("{roll}"));
            assert_eq!(decoded.roll(), roll);
            assert_eq!(decoded.pen()[..], stock[..], "{roll}");
        }
    }

    #[test]
    fn sizes_match_input_h() {
        let sizes = [
            (InputEvent::EnableHaptics, 10),
            (
                InputEvent::Keyboard {
                    key_code: 0,
                    down: true,
                    modifiers: 0,
                    flags: 0,
                },
                14,
            ),
            (InputEvent::MouseMove { dx: 0, dy: 0 }, 12),
            (
                InputEvent::MousePosition {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                18,
            ),
            (
                InputEvent::MouseButton {
                    button: 1,
                    down: true,
                },
                9,
            ),
            (InputEvent::Scroll { amount: 0 }, 14),
            (InputEvent::HScroll { amount: 0 }, 10),
            (
                InputEvent::ControllerBattery {
                    number: 0,
                    state: 0,
                    percentage: 0,
                },
                12,
            ),
            (
                InputEvent::ControllerMotion {
                    number: 0,
                    motion_type: 1,
                    x: 0.,
                    y: 0.,
                    z: 0.,
                },
                24,
            ),
            (
                InputEvent::ControllerArrival {
                    number: 0,
                    kind: 0,
                    capabilities: 0,
                    supported_buttons: 0,
                },
                16,
            ),
        ];
        for (event, size) in sizes {
            let bytes = event.encode(true);
            assert_eq!(bytes.len(), size, "{event:?}");
            assert_eq!(
                u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
                size - 4
            );
        }
    }

    /// SS_TOUCHPAD_PACKET and SS_TOUCHPAD_FRAME_PACKET of Moonlight V+'s moonlight-common-c
    /// (`Input.h`, `LiSendTouchpadEvent`, `LiSendTouchpadFrameEvent`), field by field.
    #[test]
    fn touchpad_packets_match_v_plus_input_h() {
        let single = InputEvent::Touchpad {
            event: touch_event::MOVE,
            buttons: touchpad::BUTTON_PRIMARY,
            rotation: touchpad::ROTATION_UNKNOWN,
            pointer_id: 0x0102_0304,
            width_mm: 105,
            height_mm: 70,
            x: 0.25,
            y: 0.5,
            pressure: 1.0,
            contact_major: 0.0,
            contact_minor: 0.0,
        }
        .encode(true);
        let mut expected = vec![0, 0, 0, 36, 0x09, 0, 0, 0x55, 3, 1, 0xFF, 0xFF, 4, 3, 2, 1, 105, 0, 70, 0];
        for v in [0.25f32, 0.5, 1.0, 0.0, 0.0] {
            expected.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(single, expected);

        let mut contacts = [TouchpadContact::default(); touchpad::MAX_FRAME_CONTACTS];
        contacts[0] = TouchpadContact { event: touch_event::DOWN, pointer_id: 7, x: 0.0, y: 1.0, pressure: 0.5 };
        contacts[1] = TouchpadContact { event: touch_event::UP, pointer_id: 9, x: 2.0, y: -1.0, pressure: 0.0 };
        // A contact past `count` goes as zeros whatever it holds.
        contacts[2] = TouchpadContact { event: touch_event::MOVE, pointer_id: 11, x: 0.5, y: 0.5, pressure: 0.5 };
        let frame = InputEvent::TouchpadFrame {
            count: 2,
            buttons: 0,
            rotation: touchpad::ROTATION_UNKNOWN,
            width_mm: 105,
            height_mm: 70,
            contacts,
        }
        .encode(true);
        let mut expected = vec![0, 0, 0, 72, 0x0A, 0, 0, 0x55, 2, 0, 0xFF, 0xFF, 105, 0, 70, 0];
        expected.extend_from_slice(&[1, 0, 0x00, 0x80, 7, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
        expected.extend_from_slice(&[2, 0, 0, 0, 9, 0, 0, 0, 0xFF, 0xFF, 0, 0]);
        expected.extend_from_slice(&[0; 36]);
        assert_eq!(frame, expected);
        assert_eq!(frame.len(), 76);
    }

    #[test]
    fn gfe_controllers_carry_no_extended_buttons() {
        let event = InputEvent::Controller {
            number: 0,
            active_mask: 1,
            buttons: button::A | button::PADDLE1,
            left_trigger: 0,
            right_trigger: 0,
            left_stick: (0, 0),
            right_stick: (0, 0),
        };
        let Input::Controller { buttons, .. } = decode(&event.encode(false)).unwrap() else {
            panic!("controller");
        };
        assert_eq!(buttons, button::A);
        assert_eq!(event.channel(), 0x10);
    }
}
