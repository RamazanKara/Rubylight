//! Decoding Moonlight's input packets (keyboard, mouse, pen, touch, gamepad
//! and haptics) into [`Input`] events.
use anyhow::{Result, bail};
use rubylight_protocol::steam_controller::{
    STEAM_ATTRIBUTES_BYTES, STEAM_ATTRIBUTES_MAGIC, SteamAttributes, SteamError,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Relative {
        x: i16,
        y: i16,
    },
    Absolute {
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
        horizontal: bool,
    },
    Keyboard {
        key: u16,
        modifiers: u8,
        flags: u8,
        down: bool,
    },
    Text(String),
    Controller {
        id: u16,
        active: u16,
        buttons: u32,
        left_trigger: u8,
        right_trigger: u8,
        sticks: [i16; 4],
    },
    Arrival {
        id: u8,
        kind: u8,
        capabilities: u16,
        buttons: u32,
    },
    Touch {
        event: u8,
        id: u32,
        x: f32,
        y: f32,
        pressure: f32,
        major: f32,
        minor: f32,
        rotation: u16,
    },
    Pen {
        event: u8,
        tool: u8,
        buttons: u8,
        x: f32,
        y: f32,
        pressure: f32,
        rotation: u16,
        tilt: u8,
        /// Barrel roll (twist), degrees 0-359 clockwise, or `u16::MAX` when unknown: only
        /// 0x5500000B carries it.
        roll: u16,
    },
    ControllerTouch {
        id: u8,
        event: u8,
        touchpad: u8,
        pointer: u32,
        x: f32,
        y: f32,
        pressure: f32,
    },
    Motion {
        id: u8,
        kind: u8,
        xyz: [f32; 3],
    },
    Battery {
        id: u8,
        state: u8,
        percent: u8,
    },
    Haptics(bool),
    /// The real Steam Controller's firmware attributes (0x55000008), sent
    /// after its arrival. Nothing consumes them until a driver with the
    /// Steam Controller profile is pinned.
    SteamAttributes(SteamAttributes),
}
/// A Steam Controller attributes packet (0x55000008) the host refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamAttributesError {
    /// Not the codec's 84 bytes, header included.
    Length { len: usize },
    /// A controller number past the 16 the protocol has.
    Controller { id: u8 },
    /// The shared codec refused the bytes.
    Codec(SteamError),
}
impl std::fmt::Display for SteamAttributesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Length { len } => write!(
                f,
                "Steam Controller attributes packet of {len} bytes, not {STEAM_ATTRIBUTES_BYTES}"
            ),
            Self::Controller { id } => {
                write!(f, "Steam Controller attributes for controller {id}")
            }
            Self::Codec(error) => write!(f, "Steam Controller attributes refused: {error:?}"),
        }
    }
}
impl std::error::Error for SteamAttributesError {}
/// The caller warns about a refusal once per client: one that sends the
/// attributes malformed sends them with every arrival.
fn steam_attributes(b: &[u8]) -> Result<Input> {
    if b.len() != STEAM_ATTRIBUTES_BYTES {
        bail!(SteamAttributesError::Length { len: b.len() });
    }
    match SteamAttributes::decode(b) {
        Ok(attributes) if attributes.controller >= 16 => bail!(SteamAttributesError::Controller {
            id: attributes.controller,
        }),
        Ok(attributes) => Ok(Input::SteamAttributes(attributes)),
        Err(error) => bail!(SteamAttributesError::Codec(error)),
    }
}
fn be16(b: &[u8], n: usize) -> i16 {
    i16::from_be_bytes([b[n], b[n + 1]])
}
fn le16(b: &[u8], n: usize) -> u16 {
    u16::from_le_bytes([b[n], b[n + 1]])
}
fn le32(b: &[u8], n: usize) -> u32 {
    u32::from_le_bytes(b[n..n + 4].try_into().unwrap())
}
fn float(b: &[u8], n: usize) -> Result<f32> {
    let f = f32::from_le_bytes(b[n..n + 4].try_into().unwrap());
    if !f.is_finite() {
        bail!("non-finite input coordinate");
    }
    Ok(f)
}
pub fn decode(b: &[u8]) -> Result<Input> {
    if b.len() < 8 {
        bail!("short input header");
    }
    // Before the framing check, so every wrong length is the typed error.
    if le32(b, 4) == STEAM_ATTRIBUTES_MAGIC {
        return steam_attributes(b);
    }
    let declared = u32::from_be_bytes(b[..4].try_into().unwrap()) as usize;
    if declared != b.len() - 4 {
        bail!("input length does not match header");
    }
    let magic = le32(b, 4);
    let need = match magic {
        3 | 4 => 14,
        5 => 18,
        6 | 7 => 12,
        8 | 9 => 9,
        10 => 14,
        12 => 34,
        13 => 10,
        23 => 9,
        0x55000001 => 10,
        0x55000002 => 36,
        0x55000003 => 36,
        0x5500000B => 40,
        0x55000004 => 16,
        0x55000005 => 28,
        0x55000006 => 24,
        0x55000007 => 12,
        _ => bail!("unknown input magic {magic:#x}"),
    };
    if b.len() < need {
        bail!("short input packet");
    }
    if matches!(magic, 0x55000004..=0x55000007) && b[8] >= 16 {
        bail!("invalid controller number");
    }
    if matches!(magic, 0x55000002 | 0x55000003 | 0x5500000B) && b[8] > 7 {
        bail!("invalid pointer event");
    }
    if matches!(magic, 0x55000003 | 0x5500000B) && b[9] > 2 {
        bail!("invalid pen tool");
    }
    if magic == 0x5500000B && !matches!(le16(b, 36), 0..360 | u16::MAX) {
        bail!("invalid pen barrel roll");
    }
    if magic == 0x55000005 && b[9] > 7 {
        bail!("invalid controller touch event");
    }
    if magic == 0x55000006 && !matches!(b[9], 1 | 2) {
        bail!("invalid motion sensor");
    }
    if magic == 0x55000007 && (b[9] > 5 || (b[10] > 100 && b[10] != 255)) {
        bail!("invalid battery report");
    }
    Ok(match magic {
        // Moonlight sets 0x8000 on every key code; only the low byte is the VK.
        3 | 4 => Input::Keyboard {
            key: le16(b, 9) & 0x00ff,
            modifiers: b[11],
            flags: b[8],
            down: magic == 3,
        },
        5 => {
            let (w, h) = (be16(b, 14), be16(b, 16));
            if w <= 0 || h <= 0 {
                bail!("invalid absolute mouse dimensions");
            }
            Input::Absolute {
                x: be16(b, 8),
                y: be16(b, 10),
                width: w,
                height: h,
            }
        }
        6 | 7 => Input::Relative {
            x: be16(b, 8),
            y: be16(b, 10),
        },
        8 | 9 => {
            if !(1..=5).contains(&b[8]) {
                bail!("invalid mouse button");
            }
            Input::MouseButton {
                button: b[8],
                down: magic == 8,
            }
        }
        10 => Input::Scroll {
            amount: be16(b, 8),
            horizontal: false,
        },
        12 => {
            let id = le16(b, 10);
            if id >= 16 {
                bail!("invalid controller number");
            }
            Input::Controller {
                id,
                active: le16(b, 12),
                buttons: u32::from(le16(b, 16)) | (u32::from(le16(b, 30)) << 16),
                left_trigger: b[18],
                right_trigger: b[19],
                sticks: [
                    le16(b, 20) as i16,
                    le16(b, 22) as i16,
                    le16(b, 24) as i16,
                    le16(b, 26) as i16,
                ],
            }
        }
        13 => Input::Haptics(le16(b, 8) != 0),
        23 => {
            if b.len() - 8 > 32 {
                bail!("text input too long");
            }
            Input::Text(
                std::str::from_utf8(&b[8..])?
                    .trim_end_matches('\0')
                    .to_owned(),
            )
        }
        0x55000001 => Input::Scroll {
            amount: be16(b, 8),
            horizontal: true,
        },
        0x55000002 => Input::Touch {
            event: b[8],
            id: le32(b, 12),
            x: float(b, 16)?,
            y: float(b, 20)?,
            // VoidLink can divide by zero when deriving finger pressure.
            // Zero means unknown pressure; retain the valid touch transition.
            pressure: float(b, 24).unwrap_or(0.),
            major: float(b, 28)?,
            minor: float(b, 32)?,
            rotation: le16(b, 10),
        },
        // 0x5500000B is the pen packet followed by the barrel roll.
        0x55000003 | 0x5500000B => Input::Pen {
            event: b[8],
            tool: b[9],
            buttons: b[10],
            x: float(b, 12)?,
            y: float(b, 16)?,
            pressure: float(b, 20)?,
            rotation: le16(b, 24),
            tilt: b[26],
            roll: if magic == 0x5500000B {
                le16(b, 36)
            } else {
                u16::MAX
            },
        },
        0x55000004 => {
            if b[8] >= 16 {
                bail!("invalid controller number");
            }
            Input::Arrival {
                id: b[8],
                kind: b[9],
                capabilities: le16(b, 10),
                buttons: le32(b, 12),
            }
        }
        0x55000005 => Input::ControllerTouch {
            id: b[8],
            event: b[9],
            touchpad: b[11],
            pointer: le32(b, 12),
            x: float(b, 16)?,
            y: float(b, 20)?,
            pressure: float(b, 24)?,
        },
        0x55000006 => Input::Motion {
            id: b[8],
            kind: b[9],
            xyz: [float(b, 12)?, float(b, 16)?, float(b, 20)?],
        },
        0x55000007 => Input::Battery {
            id: b[8],
            state: b[9],
            percent: b[10],
        },
        _ => unreachable!(),
    })
}
#[derive(Debug, PartialEq, Eq)]
pub enum Batch {
    Merged,
    Skip,
    Stop,
}
/// The device permission for keyboard and text input, and for its keyboard following the
/// host's text fields (0x553B).
pub const KEYBOARD_PERMISSION: u32 = 1 << 12;
impl Input {
    /// Only coalesce motion. Button, key and gesture transitions retain their order.
    pub fn merge(&mut self, newer: &Self) -> Batch {
        use Input::*;
        match (self, newer) {
            (Relative { x, y }, Relative { x: nx, y: ny }) => {
                match (x.checked_add(*nx), y.checked_add(*ny)) {
                    (Some(a), Some(b)) => {
                        *x = a;
                        *y = b;
                        Batch::Merged
                    }
                    _ => Batch::Stop,
                }
            }
            (
                Scroll { amount, horizontal },
                Scroll {
                    amount: n,
                    horizontal: h,
                },
            ) if horizontal == h => match amount.checked_add(*n) {
                Some(a) => {
                    *amount = a;
                    Batch::Merged
                }
                None => Batch::Stop,
            },
            (s @ Absolute { .. }, n @ Absolute { .. }) => {
                if let (
                    Absolute {
                        width: w,
                        height: h,
                        ..
                    },
                    Absolute {
                        width: nw,
                        height: nh,
                        ..
                    },
                ) = (&*s, n)
                    && (w != nw || h != nh)
                {
                    return Batch::Stop;
                }
                *s = n.clone();
                Batch::Merged
            }
            (s @ Controller { .. }, n @ Controller { .. }) => {
                if let (
                    Controller {
                        id,
                        active,
                        buttons,
                        ..
                    },
                    Controller {
                        id: ni,
                        active: na,
                        buttons: nb,
                        ..
                    },
                ) = (&*s, n)
                {
                    if active != na {
                        return Batch::Stop;
                    }
                    if id != ni {
                        return Batch::Skip;
                    }
                    if buttons != nb {
                        return Batch::Stop;
                    }
                }
                *s = n.clone();
                Batch::Merged
            }
            (s @ Touch { .. }, n @ Touch { .. }) => {
                if let (
                    Touch { event, id, .. },
                    Touch {
                        event: ne, id: ni, ..
                    },
                ) = (&*s, n)
                {
                    if !matches!(event, 0 | 3) || !matches!(ne, 0 | 3) {
                        return Batch::Stop;
                    }
                    if id != ni {
                        return Batch::Skip;
                    }
                    if event != ne {
                        return Batch::Stop;
                    }
                }
                *s = n.clone();
                Batch::Merged
            }
            (s @ Pen { .. }, n @ Pen { .. }) => {
                if let (
                    Pen {
                        event,
                        tool,
                        buttons,
                        ..
                    },
                    Pen {
                        event: ne,
                        tool: nt,
                        buttons: nb,
                        ..
                    },
                ) = (&*s, n)
                    && (!matches!(event, 0 | 3) || event != ne || tool != nt || buttons != nb)
                {
                    return Batch::Stop;
                }
                *s = n.clone();
                Batch::Merged
            }
            (s @ Motion { .. }, n @ Motion { .. }) => {
                if let (
                    Motion { id, kind, .. },
                    Motion {
                        id: ni, kind: nk, ..
                    },
                ) = (&*s, n)
                    && (id != ni || kind != nk)
                {
                    return Batch::Skip;
                }
                *s = n.clone();
                Batch::Merged
            }
            (s @ ControllerTouch { .. }, n @ ControllerTouch { .. }) => {
                if let (
                    ControllerTouch {
                        id,
                        event,
                        touchpad,
                        pointer,
                        ..
                    },
                    ControllerTouch {
                        id: ni,
                        event: ne,
                        touchpad: nt,
                        pointer: np,
                        ..
                    },
                ) = (&*s, n)
                {
                    if id != ni || touchpad != nt {
                        return Batch::Skip;
                    }
                    // Cancel-all affects every contact on this touchpad, even
                    // if its pointer ID differs from the move being batched.
                    if *event == 7 || *ne == 7 {
                        return Batch::Stop;
                    }
                    if pointer != np {
                        return Batch::Skip;
                    }
                    if !matches!(event, 0 | 3) || event != ne {
                        return Batch::Stop;
                    }
                }
                *s = n.clone();
                Batch::Merged
            }
            _ => Batch::Stop,
        }
    }
    pub fn required_permission(&self) -> u32 {
        match self {
            Self::Keyboard { .. } | Self::Text(_) => KEYBOARD_PERMISSION,
            Self::Touch { .. } => 1 << 9,
            Self::Pen { .. } => 1 << 10,
            Self::Relative { .. }
            | Self::Absolute { .. }
            | Self::MouseButton { .. }
            | Self::Scroll { .. } => 1 << 11,
            _ => 1 << 8,
        }
    }
    /// The stream-card warning for input this device may not send, naming
    /// the permission as the console's device page does.
    pub fn denied_warning(&self, device: &str) -> (String, String) {
        let (code, name) = match self.required_permission() {
            0x1000 => ("keyboard", "Keyboard"),
            0x200 => ("touch", "Touch"),
            0x400 => ("pen", "Pen"),
            0x800 => ("mouse", "Mouse"),
            _ => ("controller", "Controllers"),
        };
        (
            format!("input_permission_{code}"),
            format!(
                "{device} sends {code} input, but its device permissions do not allow it, so the host ignores it. Turn on {name} for this device under Devices in the console, then reconnect."
            ),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denied_input_names_the_device_permission_to_turn_on() {
        let pad = Input::Controller {
            id: 0,
            active: 1,
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        };
        let (code, message) = pad.denied_warning("Pixel 8");
        assert_eq!(code, "input_permission_controller");
        assert!(message.starts_with("Pixel 8 sends controller input"));
        assert!(message.contains("Turn on Controllers"));
        // Arrivals, motion and haptics share the controller permission and warning.
        for event in [
            Input::Arrival {
                id: 0,
                kind: 1,
                capabilities: 0,
                buttons: 0,
            },
            Input::Haptics(true),
        ] {
            assert_eq!(event.required_permission(), 1 << 8);
            assert_eq!(
                event.denied_warning("Pixel 8"),
                (code.clone(), message.clone())
            );
        }
        let (code, message) = Input::Text("a".into()).denied_warning("Tablet");
        assert_eq!(code, "input_permission_keyboard");
        assert!(message.contains("sends keyboard input") && message.contains("Turn on Keyboard"));
        let (code, message) = Input::Relative { x: 1, y: 1 }.denied_warning("Tablet");
        assert_eq!(code, "input_permission_mouse");
        assert!(message.contains("Turn on Mouse"));
    }
    #[test]
    fn overflow_leaves_both_axes_untouched() {
        let mut a = Input::Relative {
            x: 100,
            y: i16::MAX,
        };
        let b = Input::Relative { x: 1, y: 1 };
        assert_eq!(a.merge(&b), Batch::Stop);
        assert_eq!(
            a,
            Input::Relative {
                x: 100,
                y: i16::MAX
            }
        );
        assert_eq!(b, Input::Relative { x: 1, y: 1 });
    }
    #[test]
    fn relative_negative_and_scroll_merge() {
        let mut a = Input::Relative { x: -200, y: 50 };
        assert_eq!(a.merge(&Input::Relative { x: 5, y: -4 }), Batch::Merged);
        assert_eq!(a, Input::Relative { x: -195, y: 46 });
    }
    #[test]
    fn never_swallow_button_transition() {
        let mut a = Input::MouseButton {
            button: 1,
            down: true,
        };
        assert_eq!(
            a.merge(&Input::MouseButton {
                button: 1,
                down: false
            }),
            Batch::Stop
        );
    }
    #[test]
    fn malformed_packets_never_panic() {
        for len in 0..96 {
            for magic in [
                3u32,
                4,
                5,
                7,
                8,
                10,
                12,
                23,
                0x55000002,
                0x55000003,
                0x55000005,
                0x55000006,
                0x5500000B,
                STEAM_ATTRIBUTES_MAGIC,
                u32::MAX,
            ] {
                let mut b = vec![0; len];
                if len >= 8 {
                    b[..4].copy_from_slice(&((len - 4) as u32).to_be_bytes());
                    b[4..8].copy_from_slice(&magic.to_le_bytes());
                }
                let _ = decode(&b);
            }
        }
    }
    /// 0x5500000B is the pen packet with the barrel roll after it (`rubylight_protocol::pen`);
    /// the stock pen packet leaves the roll unknown.
    #[test]
    fn pen_barrel_roll_decodes_beside_the_stock_pen() {
        use rubylight_protocol::pen::{PEN_BYTES, PenBarrelRoll, ROLL_UNKNOWN};
        let mut pen = [0u8; PEN_BYTES];
        pen[..4].copy_from_slice(&((PEN_BYTES - 4) as u32).to_be_bytes());
        pen[4..8].copy_from_slice(&0x5500_0003u32.to_le_bytes());
        pen[8] = 1;
        pen[9] = 1;
        pen[12..16].copy_from_slice(&0.25f32.to_le_bytes());
        pen[16..20].copy_from_slice(&0.75f32.to_le_bytes());
        pen[20..24].copy_from_slice(&0.5f32.to_le_bytes());
        pen[24..26].copy_from_slice(&90u16.to_le_bytes());
        pen[26] = 30;
        let stock = decode(&pen).unwrap();
        let Input::Pen { roll, .. } = stock else {
            panic!("{stock:?}");
        };
        assert_eq!(roll, u16::MAX);
        for value in [0, 270, 359, ROLL_UNKNOWN] {
            let wire = PenBarrelRoll::new(&pen, value).unwrap().encode();
            let Input::Pen {
                event,
                tool,
                x,
                y,
                pressure,
                rotation,
                tilt,
                roll,
                ..
            } = decode(&wire).unwrap()
            else {
                panic!("not a pen");
            };
            assert_eq!(
                (event, tool, x, y, pressure, rotation, tilt, roll),
                (1, 1, 0.25, 0.75, 0.5, 90, 30, value)
            );
        }
        // A roll past 359 that is not unknown, a short packet and the pen's own checks
        // refuse it.
        let mut wire = PenBarrelRoll::new(&pen, 10).unwrap().encode();
        wire[36..38].copy_from_slice(&360u16.to_le_bytes());
        assert!(decode(&wire).is_err());
        let mut short = PenBarrelRoll::new(&pen, 10).unwrap().encode().to_vec();
        short.truncate(38);
        short[..4].copy_from_slice(&34u32.to_be_bytes());
        assert!(decode(&short).is_err());
        let mut tool = PenBarrelRoll::new(&pen, 10).unwrap().encode();
        tool[9] = 3;
        assert!(decode(&tool).is_err());
    }
    #[test]
    fn keyboard_codes_drop_moonlights_high_byte() {
        // Left arrow (VK 0x25) with Shift, as moonlight-qt and Artemis send it.
        let p = hex::decode("0000000a03000000002580010000").unwrap();
        assert_eq!(
            decode(&p).unwrap(),
            Input::Keyboard {
                key: 0x25,
                modifiers: 1,
                flags: 0,
                down: true
            }
        );
    }
    fn controller_touch(touchpad: u8, event: u8, pointer: u32, x: f32) -> Input {
        Input::ControllerTouch {
            id: 2,
            event,
            touchpad,
            pointer,
            x,
            y: 0.5,
            pressure: 0.75,
        }
    }
    #[test]
    fn controller_touch_decodes_the_wire_touchpad_index_without_changing_coordinates() {
        for touchpad in [0, 1, 2, 255] {
            let mut packet = vec![0; 28];
            packet[..4].copy_from_slice(&24u32.to_be_bytes());
            packet[4..8].copy_from_slice(&0x55000005u32.to_le_bytes());
            packet[8] = 2;
            packet[9] = 3;
            packet[11] = touchpad;
            packet[12..16].copy_from_slice(&17u32.to_le_bytes());
            packet[16..20].copy_from_slice(&0.25f32.to_le_bytes());
            packet[20..24].copy_from_slice(&0.5f32.to_le_bytes());
            packet[24..28].copy_from_slice(&0.75f32.to_le_bytes());
            assert_eq!(
                decode(&packet).unwrap(),
                controller_touch(touchpad, 3, 17, 0.25)
            );
        }
    }
    #[test]
    fn controller_touch_coalescing_keeps_surfaces_and_cancel_all_separate() {
        let mut primary = controller_touch(0, 3, 17, 0.25);
        let original = primary.clone();
        assert_eq!(primary.merge(&controller_touch(1, 3, 17, 0.9)), Batch::Skip);
        assert_eq!(primary, original);
        assert_eq!(primary.merge(&controller_touch(1, 7, 0, 0.0)), Batch::Skip);
        assert_eq!(primary.merge(&controller_touch(0, 7, 0, 0.0)), Batch::Stop);
        assert_eq!(primary, original);
        assert_eq!(primary.merge(&controller_touch(0, 2, 17, 0.0)), Batch::Stop);
        assert_eq!(
            primary.merge(&controller_touch(0, 3, 17, 0.5)),
            Batch::Merged
        );
        assert_eq!(primary, controller_touch(0, 3, 17, 0.5));
        let mut cancelled = controller_touch(0, 7, 0, 0.0);
        assert_eq!(cancelled.merge(&primary), Batch::Stop);
    }

    #[test]
    fn actual_moonlight_mouse_packet() {
        let p = hex::decode("0000000807000000fffb0004").unwrap();
        assert_eq!(decode(&p).unwrap(), Input::Relative { x: -5, y: 4 });
    }

    fn native_touch_packet(event: u8, pressure: f32) -> Vec<u8> {
        let mut packet = vec![0; 36];
        packet[..4].copy_from_slice(&32u32.to_be_bytes());
        packet[4..8].copy_from_slice(&0x55000002u32.to_le_bytes());
        packet[8] = event;
        packet[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
        packet[12..16].copy_from_slice(&7u32.to_le_bytes());
        packet[16..20].copy_from_slice(&0.25f32.to_le_bytes());
        packet[20..24].copy_from_slice(&0.75f32.to_le_bytes());
        packet[24..28].copy_from_slice(&pressure.to_le_bytes());
        packet
    }

    #[test]
    fn native_touch_preserves_transitions_with_unknown_pressure() {
        for event in [1, 3, 2] {
            for pressure in [0., 0.5, 1., f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                assert_eq!(
                    decode(&native_touch_packet(event, pressure)).unwrap(),
                    Input::Touch {
                        event,
                        id: 7,
                        x: 0.25,
                        y: 0.75,
                        pressure: if pressure.is_finite() { pressure } else { 0. },
                        major: 0.,
                        minor: 0.,
                        rotation: u16::MAX,
                    }
                );
            }
        }
    }

    /// The codec's golden attributes packet, byte for byte.
    fn steam_attributes_packet() -> Vec<u8> {
        let mut packet = vec![0; 84];
        packet[..8].copy_from_slice(&[0, 0, 0, 80, 0x08, 0x00, 0x00, 0x55]);
        packet[8..12].copy_from_slice(&[1, 2, 0xff, 0]);
        packet[12..16].copy_from_slice(&[0xe3, 0x85, 0x4d, 0x6a]);
        packet[16..20].copy_from_slice(&[0x2e, 0xf9, 0xd2, 0x68]);
        packet[20..24].copy_from_slice(&[74, 0, 0, 0]);
        packet[24..28].copy_from_slice(&[0x04, 0x03, 0x02, 0x01]);
        packet[28..32].copy_from_slice(&[0x14, 0x13, 0x12, 0x11]);
        packet[32..36].copy_from_slice(&[0x24, 0x23, 0x22, 0x21]);
        packet[36..48].copy_from_slice(b"26e1f7ef1955");
        packet[52..59].copy_from_slice(b"FXA0001");
        packet[68..75].copy_from_slice(b"MXA0002");
        packet
    }

    fn steam_attributes_error(packet: &[u8]) -> SteamAttributesError {
        *decode(packet)
            .unwrap_err()
            .downcast_ref::<SteamAttributesError>()
            .expect("typed Steam attributes error")
    }

    #[test]
    fn steam_attributes_decode_the_codecs_golden_packet() {
        let Input::SteamAttributes(attributes) = decode(&steam_attributes_packet()).unwrap() else {
            panic!("not Steam attributes");
        };
        let padded = |text: &[u8]| {
            let mut out = [0; 16];
            out[..text.len()].copy_from_slice(text);
            out
        };
        assert_eq!(
            attributes,
            SteamAttributes {
                controller: 1,
                transport: rubylight_protocol::steam_controller::LI_SCATTR_TRANSPORT_PUCK,
                valid_mask: 0xff,
                firmware_build_time: 0x6a4d_85e3,
                bootloader_build_time: 0x68d2_f92e,
                board_revision: 74,
                capabilities: 0x0102_0304,
                radio_firmware_build_time: 0x1112_1314,
                dongle_firmware_build_time: 0x2122_2324,
                build_id: padded(b"26e1f7ef1955"),
                unit_serial: padded(b"FXA0001"),
                board_serial: padded(b"MXA0002"),
            }
        );
        // Shares the controller permission; never coalesced with anything.
        let event = Input::SteamAttributes(attributes);
        assert_eq!(event.required_permission(), 1 << 8);
        assert_eq!(event.clone().merge(&event), Batch::Stop);
    }

    #[test]
    fn steam_attributes_of_the_wrong_length_are_a_typed_error() {
        let golden = steam_attributes_packet();
        for len in [8, 12, 80, 83, 85, 120] {
            let mut packet = golden.clone();
            packet.resize(len, 0);
            // A consistent framing header must not let a wrong length through.
            packet[..4].copy_from_slice(&((len - 4) as u32).to_be_bytes());
            assert_eq!(
                steam_attributes_error(&packet),
                SteamAttributesError::Length { len }
            );
            packet[..4].copy_from_slice(&80u32.to_be_bytes());
            assert_eq!(
                steam_attributes_error(&packet),
                SteamAttributesError::Length { len }
            );
        }
        let mut header = golden.clone();
        header[..4].copy_from_slice(&84u32.to_be_bytes());
        assert_eq!(
            steam_attributes_error(&header),
            SteamAttributesError::Codec(SteamError::BadHeader {
                length: 84,
                magic: STEAM_ATTRIBUTES_MAGIC
            })
        );
        let mut controller = golden;
        controller[8] = 16;
        assert_eq!(
            steam_attributes_error(&controller),
            SteamAttributesError::Controller { id: 16 }
        );
    }

    #[test]
    fn refused_or_unknown_steam_packets_leave_other_input_alone() {
        let mouse = hex::decode("0000000807000000fffb0004").unwrap();
        let mut unknown = steam_attributes_packet();
        unknown[4] = 0x09;
        for refused in [vec![0, 0, 0, 4, 0x08, 0, 0, 0x55], unknown] {
            assert!(decode(&refused).is_err());
            assert_eq!(decode(&mouse).unwrap(), Input::Relative { x: -5, y: 4 });
        }
    }

    #[test]
    fn native_touch_still_rejects_nonfinite_coordinates() {
        for offset in [16, 20] {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut packet = native_touch_packet(1, f32::NAN);
                packet[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                assert!(decode(&packet).is_err());
            }
        }
    }
}
