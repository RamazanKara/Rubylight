//! Decoding Moonlight's input packets (keyboard, mouse, pen, touch, gamepad
//! and haptics) into [`Input`] events.
use anyhow::{Result, bail};
use rubylight_protocol::controller_identity::{CONTROLLER_IDENTITY_MAGIC, ControllerIdentity};
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
    /// A tablet's physical touchpad: one finger (0x55000009, `frame` false) or every finger
    /// of a hardware frame (0x5500000A), Moonlight V+'s and Foundation Sunshine's touchpad
    /// packets. `buttons` bit 0 is the click; the size is the client's surface in millimetres,
    /// 0 when unknown. The host replays it on the virtual Precision Touchpad.
    Touchpad {
        frame: bool,
        buttons: u8,
        width_mm: u16,
        height_mm: u16,
        contacts: Vec<crate::virtual_hid::Contact>,
    },
    /// The real Steam Controller's firmware attributes (0x55000008), sent
    /// after its arrival, for the VHF Steam Controller profile.
    SteamAttributes(SteamAttributes),
    /// Which physical controller a client's gamepad is (0x55000080), sent
    /// just before its arrival by a client that read the host's
    /// `ControllerIdentityVersion`.
    ControllerIdentity(ControllerIdentity),
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
/// Moonlight V+'s and Foundation Sunshine's one-finger touchpad packet.
pub const TOUCHPAD_MAGIC: u32 = 0x5500_0009;
/// Their touchpad frame packet: up to five fingers of one hardware frame.
pub const TOUCHPAD_FRAME_MAGIC: u32 = 0x5500_000A;
pub fn decode(b: &[u8]) -> Result<Input> {
    if b.len() < 8 {
        bail!("short input header");
    }
    // Before the framing check, so every wrong length is the typed error.
    if le32(b, 4) == STEAM_ATTRIBUTES_MAGIC {
        return steam_attributes(b);
    }
    if le32(b, 4) == CONTROLLER_IDENTITY_MAGIC {
        return ControllerIdentity::decode(b)
            .map(Input::ControllerIdentity)
            .map_err(|error| anyhow::anyhow!("controller identity refused: {error:?}"));
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
        TOUCHPAD_MAGIC => 40,
        TOUCHPAD_FRAME_MAGIC => 76,
        _ => bail!("unknown input magic {magic:#x}"),
    };
    if b.len() < need {
        bail!("short input packet");
    }
    // The touchpad packets are fixed-size structs (SS_TOUCHPAD_PACKET, SS_TOUCHPAD_FRAME_PACKET):
    // another length is another packet, not a touchpad one with extra bytes.
    if matches!(magic, TOUCHPAD_MAGIC | TOUCHPAD_FRAME_MAGIC) && b.len() != need {
        bail!("touchpad packet of the wrong length");
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
    if magic == TOUCHPAD_MAGIC && b[8] > 7 {
        bail!("invalid touchpad event");
    }
    if magic == TOUCHPAD_FRAME_MAGIC
        && (usize::from(b[8]) > crate::virtual_hid::MAX_CONTACTS
            || (0..usize::from(b[8])).any(|n| b[16 + n * 12] > 7))
    {
        bail!("invalid touchpad frame");
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
        // SS_TOUCHPAD_PACKET: event, buttons, rotation, pointer id, size, then x, y,
        // pressure and contact area as floats.
        TOUCHPAD_MAGIC => Input::Touchpad {
            frame: false,
            buttons: b[9],
            width_mm: le16(b, 16),
            height_mm: le16(b, 18),
            contacts: vec![crate::virtual_hid::Contact {
                event: b[8],
                id: le32(b, 12),
                x: float(b, 20)?,
                y: float(b, 24)?,
            }],
        },
        // SS_TOUCHPAD_FRAME_PACKET: count, buttons, rotation, size, then five contacts of
        // event, reserved, pressure, pointer id, x and y (normalized u16s).
        TOUCHPAD_FRAME_MAGIC => Input::Touchpad {
            frame: true,
            buttons: b[9],
            width_mm: le16(b, 12),
            height_mm: le16(b, 14),
            contacts: (0..usize::from(b[8]))
                .map(|n| {
                    let at = 16 + n * 12;
                    crate::virtual_hid::Contact {
                        event: b[at],
                        id: le32(b, at + 4),
                        x: f32::from(le16(b, at + 8)) / 65535.0,
                        y: f32::from(le16(b, at + 10)) / 65535.0,
                    }
                })
                .collect(),
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
            (s @ Touchpad { .. }, n @ Touchpad { .. }) => {
                if let (
                    Touchpad {
                        frame,
                        buttons,
                        contacts,
                        ..
                    },
                    Touchpad {
                        frame: nf,
                        buttons: nb,
                        contacts: nc,
                        ..
                    },
                ) = (&*s, n)
                {
                    // Only moves of the same fingers with the click unchanged coalesce, as in
                    // Foundation Sunshine; a finger going down or up never does.
                    let moves = |c: &[crate::virtual_hid::Contact]| {
                        !c.is_empty() && c.iter().all(|c| c.event == 3)
                    };
                    if frame != nf
                        || buttons != nb
                        || !moves(contacts.as_slice())
                        || !moves(nc.as_slice())
                    {
                        return Batch::Stop;
                    }
                    if contacts.len() != nc.len()
                        || contacts.iter().zip(nc).any(|(a, b)| a.id != b.id)
                    {
                        return Batch::Skip;
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
            | Self::Scroll { .. }
            | Self::Touchpad { .. } => 1 << 11,
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
mod tests;
