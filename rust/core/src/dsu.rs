//! DSU (cemuhook) motion server protocol: the packets, the client
//! registrations and each streamed controller's state as the server reports
//! it. Emulators such as Cemu, Dolphin, Citra and Yuzu read a controller's
//! gyroscope and accelerometer, buttons and touch from it over UDP.
//!
//! The protocol follows DS4Windows' DSU server (`DS4Control/UdpServer.cs`,
//! Ryochan7/DS4Windows and schmaldeo/DS4Windows, GPL-3.0), the reference
//! server cemuhook clients are tested against; the setting names and default
//! port are Foundation Sunshine's (`config.enable_dsu_server`,
//! `dsu_server_port`, AlkaidLab/foundation-sunshine, GPL-3.0).
//!
//! Every packet starts with a 16-byte header: the magic (`DSUC` from a
//! client, `DSUS` from the server), the protocol version (1001), the length
//! of what follows the header, a CRC-32 of the whole packet taken with its
//! own field zeroed, and the sender's id. The message type comes next.

use crate::input::Input;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// Foundation Sunshine's `dsu_server_port` default (cemuhook's port).
pub const DEFAULT_PORT: u16 = 26760;
/// The protocol version both sides send; a newer client's packets are dropped.
pub const PROTOCOL_VERSION: u16 = 1001;
/// Controller slots a DSU server reports.
pub const SLOTS: usize = 4;
/// A client that stops asking for pad data is dropped after this long.
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);
/// Protocol version request and reply.
pub const MSG_VERSION: u32 = 0x10_0000;
/// Connected controllers (`ListPorts`) request and port info reply.
pub const MSG_PORTS: u32 = 0x10_0001;
/// Pad data subscription and the pad data the server sends.
pub const MSG_PAD_DATA: u32 = 0x10_0002;
/// A pad data packet's size, its header included.
pub const PAD_DATA_BYTES: usize = 100;
const HEADER: usize = 16;
const CLIENT_MAGIC: &[u8; 4] = b"DSUC";
const SERVER_MAGIC: &[u8; 4] = b"DSUS";
/// Standard gravity: Moonlight's accelerometer is in m/s², DSU's in g.
const GRAVITY: f32 = 9.806_65;
/// An analog trigger past this reads as pressed, for the digital L2 and R2.
const TRIGGER_THRESHOLD: u8 = 32;
/// The DualShock 4 touchpad's resolution, which DSU touch positions use.
const TOUCH_WIDTH: f32 = 1920.;
const TOUCH_HEIGHT: f32 = 943.;

/// What a client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Version,
    /// Port info for the first `count` of `slots`, each below [`SLOTS`].
    Ports {
        count: u8,
        slots: [u8; SLOTS],
    },
    /// Pad data for every slot (`flags` 0), the slot (bit 0) and/or the
    /// MAC address (bit 1), until [`CLIENT_TIMEOUT`] passes without another.
    PadData {
        flags: u8,
        slot: u8,
        mac: [u8; 6],
    },
}

/// A client packet's sender id and request; `None` for anything a DSU server
/// drops: another magic, a newer version, a short or corrupt packet, an
/// unknown message or an invalid port list.
pub fn parse(packet: &[u8]) -> Option<(u32, Request)> {
    if packet.len() < HEADER + 4 || &packet[..4] != CLIENT_MAGIC {
        return None;
    }
    if u16::from_le_bytes([packet[4], packet[5]]) > PROTOCOL_VERSION {
        return None;
    }
    // The length counts what follows the header; anything after it is ignored.
    let size = HEADER + usize::from(u16::from_le_bytes([packet[6], packet[7]]));
    if size > packet.len() || size < HEADER + 4 {
        return None;
    }
    let packet = &packet[..size];
    let crc = u32::from_le_bytes(packet[8..12].try_into().unwrap());
    if crc != checksum(packet) {
        return None;
    }
    let client = u32::from_le_bytes(packet[12..16].try_into().unwrap());
    let body = &packet[HEADER + 4..];
    let request = match u32::from_le_bytes(packet[16..20].try_into().unwrap()) {
        MSG_VERSION => Request::Version,
        MSG_PORTS => {
            let count = i32::from_le_bytes(body.get(..4)?.try_into().unwrap());
            if !(0..=SLOTS as i32).contains(&count) {
                return None;
            }
            let listed = body.get(4..4 + count as usize)?;
            if listed.iter().any(|&slot| usize::from(slot) >= SLOTS) {
                return None;
            }
            let mut slots = [0; SLOTS];
            slots[..listed.len()].copy_from_slice(listed);
            Request::Ports {
                count: count as u8,
                slots,
            }
        }
        MSG_PAD_DATA => {
            let body = body.get(..8)?;
            Request::PadData {
                flags: body[0],
                slot: body[1],
                mac: body[2..8].try_into().unwrap(),
            }
        }
        _ => return None,
    };
    Some((client, request))
}

/// CRC-32 (IEEE) of a packet with its CRC field taken as zero.
fn checksum(packet: &[u8]) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&packet[..8]);
    hasher.update(&[0; 4]);
    hasher.update(&packet[12..]);
    hasher.finalize()
}

/// A server packet: header, `message` and `payload`, its length and CRC set.
fn packet(server: u32, message: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + 4 + payload.len());
    out.extend_from_slice(SERVER_MAGIC);
    out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    out.extend_from_slice(&((4 + payload.len()) as u16).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&server.to_le_bytes());
    out.extend_from_slice(&message.to_le_bytes());
    out.extend_from_slice(payload);
    let crc = checksum(&out);
    out[8..12].copy_from_slice(&crc.to_le_bytes());
    out
}

/// The reply to [`Request::Version`].
pub fn version_reply(server: u32) -> Vec<u8> {
    let mut payload = PROTOCOL_VERSION.to_le_bytes().to_vec();
    payload.extend_from_slice(&[0, 0]);
    packet(server, MSG_VERSION, &payload)
}

/// The 11 bytes that describe a slot in port info and pad data: slot, state
/// (0 not connected, 2 connected), model (2: full gyro), connection (1 USB),
/// MAC address and battery.
fn slot_info(slot: u8, pad: Option<&Pad>) -> [u8; 11] {
    let mut out = [0; 11];
    out[0] = slot;
    if let Some(pad) = pad {
        out[1] = 2;
        out[2] = 2;
        out[3] = 1;
        out[4..10].copy_from_slice(&mac(slot));
        out[10] = pad.battery_status();
    }
    out
}

/// The MAC address a slot reports, so clients that subscribe by address can.
pub fn mac(slot: u8) -> [u8; 6] {
    [0, 0, 0, 0, 0, slot + 1]
}

/// The reply to [`Request::Ports`] for one slot, connected or not.
pub fn port_info(server: u32, slot: u8, pad: Option<&Pad>) -> Vec<u8> {
    let mut payload = slot_info(slot, pad).to_vec();
    payload.push(0);
    packet(server, MSG_PORTS, &payload)
}

/// One slot's pad data, the `number`th packet sent for it.
pub fn pad_data(server: u32, slot: u8, pad: &Pad, number: u32) -> Vec<u8> {
    let mut p = Vec::with_capacity(PAD_DATA_BYTES - HEADER - 4);
    p.extend_from_slice(&slot_info(slot, Some(pad)));
    p.push(1);
    p.extend_from_slice(&number.to_le_bytes());
    let (b1, b2) = pad.dsu_buttons();
    p.extend_from_slice(&[b1, b2]);
    p.push(u8::from(pad.buttons & button::SPECIAL != 0));
    p.push(u8::from(pad.buttons & button::TOUCHPAD != 0));
    let axis = |v: i16| ((i32::from(v) + 32768) >> 8) as u8;
    // Moonlight's Y is up-positive; DSU's inverted DualShock 4 Y is too (255 up).
    p.extend_from_slice(&[
        axis(pad.sticks[0]),
        axis(pad.sticks[1]),
        axis(pad.sticks[2]),
        axis(pad.sticks[3]),
    ]);
    let analog = |flag: u32| if pad.buttons & flag != 0 { 0xFF } else { 0 };
    p.extend_from_slice(&[
        analog(button::LEFT),
        analog(button::DOWN),
        analog(button::RIGHT),
        analog(button::UP),
        analog(button::X),
        analog(button::A),
        analog(button::B),
        analog(button::Y),
        analog(button::RB),
        analog(button::LB),
        pad.triggers[1],
        pad.triggers[0],
    ]);
    for touch in &pad.touches {
        let (active, id, x, y) = touch.map_or((0, 0, 0, 0), |t| {
            let at = |v: f32, size: f32| (v.clamp(0., 1.) * (size - 1.)).round() as u16;
            (1, t.id, at(t.x, TOUCH_WIDTH), at(t.y, TOUCH_HEIGHT))
        });
        p.extend_from_slice(&[active, id]);
        p.extend_from_slice(&x.to_le_bytes());
        p.extend_from_slice(&y.to_le_bytes());
    }
    p.extend_from_slice(&pad.motion_us.to_le_bytes());
    for v in pad.dsu_accel().into_iter().chain(pad.dsu_gyro()) {
        p.extend_from_slice(&v.to_le_bytes());
    }
    packet(server, MSG_PAD_DATA, &p)
}

/// Moonlight button flags (`LI_*_FLAG`).
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
    pub const TOUCHPAD: u32 = 0x10_0000;
}

/// A touchpad contact: Moonlight's 0-1 position and the id DSU reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Touch {
    pub pointer: u32,
    pub id: u8,
    pub x: f32,
    pub y: f32,
}

/// One streamed controller as the server reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    pub buttons: u32,
    /// Left and right trigger.
    pub triggers: [u8; 2],
    /// Left X, left Y, right X, right Y, Moonlight's (Y up-positive).
    pub sticks: [i16; 4],
    /// SDL frame, m/s² (Moonlight's `LI_MOTION_TYPE_ACCEL`).
    pub accel: [f32; 3],
    /// SDL frame, degrees per second (`LI_MOTION_TYPE_GYRO`).
    pub gyro: [f32; 3],
    /// When the last motion reading came, in microseconds on the server's clock.
    pub motion_us: u64,
    pub touches: [Option<Touch>; 2],
    /// Moonlight's battery state and percent (`LI_BATTERY_*`).
    pub battery: (u8, u8),
    next_touch_id: u8,
}

impl Default for Pad {
    fn default() -> Self {
        Self {
            buttons: 0,
            triggers: [0; 2],
            sticks: [0; 4],
            accel: [0., GRAVITY, 0.],
            gyro: [0.; 3],
            motion_us: 0,
            touches: [None; 2],
            battery: (0, 0xFF),
            next_touch_id: 0,
        }
    }
}

/// `LI_TOUCH_EVENT_*`.
mod touch_event {
    pub const DOWN: u8 = 0x01;
    pub const UP: u8 = 0x02;
    pub const MOVE: u8 = 0x03;
    pub const CANCEL: u8 = 0x04;
    pub const CANCEL_ALL: u8 = 0x07;
}

impl Pad {
    /// Applies a controller, touch, motion or battery event of this pad;
    /// `motion_us` stamps a motion reading. Returns whether anything changed.
    pub fn apply(&mut self, input: &Input, motion_us: u64) -> bool {
        let before = self.clone();
        match *input {
            Input::Controller {
                buttons,
                left_trigger,
                right_trigger,
                sticks,
                ..
            } => {
                self.buttons = buttons;
                self.triggers = [left_trigger, right_trigger];
                self.sticks = sticks;
            }
            Input::Motion { kind: 1, xyz, .. } => {
                self.accel = xyz;
                self.motion_us = motion_us;
            }
            Input::Motion { kind: 2, xyz, .. } => {
                self.gyro = xyz;
                self.motion_us = motion_us;
            }
            Input::Battery { state, percent, .. } => self.battery = (state, percent),
            Input::ControllerTouch {
                event,
                touchpad: 0,
                pointer,
                x,
                y,
                ..
            } => self.touch(event, pointer, x, y),
            _ => {}
        }
        *self != before
    }

    fn touch(&mut self, event: u8, pointer: u32, x: f32, y: f32) {
        let held = self
            .touches
            .iter()
            .position(|t| t.is_some_and(|t| t.pointer == pointer));
        match event {
            touch_event::CANCEL_ALL => self.touches = [None; 2],
            touch_event::UP | touch_event::CANCEL => {
                if let Some(i) = held {
                    self.touches[i] = None;
                }
            }
            touch_event::DOWN | touch_event::MOVE => {
                if let Some(i) = held {
                    let t = self.touches[i].as_mut().unwrap();
                    (t.x, t.y) = (x, y);
                } else if let Some(i) = self.touches.iter().position(Option::is_none) {
                    // A new contact gets a new id, as a DualShock 4's does.
                    self.touches[i] = Some(Touch {
                        pointer,
                        id: self.next_touch_id,
                        x,
                        y,
                    });
                    self.next_touch_id = self.next_touch_id.wrapping_add(1);
                }
            }
            _ => {}
        }
    }

    /// DSU's two button bytes: d-pad, Options, R3, L3 and Share, then the
    /// face buttons by position (Moonlight's A is Cross), shoulders and the
    /// digital triggers.
    fn dsu_buttons(&self) -> (u8, u8) {
        let bit = |flag: u32, value: u8| if self.buttons & flag != 0 { value } else { 0 };
        let first = bit(button::LEFT, 0x80)
            | bit(button::DOWN, 0x40)
            | bit(button::RIGHT, 0x20)
            | bit(button::UP, 0x10)
            | bit(button::PLAY, 0x08)
            | bit(button::RS_CLK, 0x04)
            | bit(button::LS_CLK, 0x02)
            | bit(button::BACK, 0x01);
        let mut second = bit(button::X, 0x80)
            | bit(button::A, 0x40)
            | bit(button::B, 0x20)
            | bit(button::Y, 0x10)
            | bit(button::RB, 0x08)
            | bit(button::LB, 0x04);
        if self.triggers[1] > TRIGGER_THRESHOLD {
            second |= 0x02;
        }
        if self.triggers[0] > TRIGGER_THRESHOLD {
            second |= 0x01;
        }
        (first, second)
    }

    /// The accelerometer in g on DSU's axes. SDL's frame is the DualShock 4's
    /// raw one; DS4Windows reports that negated (`DS4Sixaxis.cs`), so a pad
    /// lying flat reads -1 g on Y.
    pub fn dsu_accel(&self) -> [f32; 3] {
        self.accel.map(|v| -v / GRAVITY)
    }

    /// The gyroscope in degrees per second as pitch, yaw and roll
    /// (DS4Windows: pitch is the raw X rate, yaw and roll the negated Y and Z).
    pub fn dsu_gyro(&self) -> [f32; 3] {
        [self.gyro[0], -self.gyro[1], -self.gyro[2]]
    }

    /// DSU's battery byte: 0xEE charging, 0xEF charged, else a level from 1
    /// (dying) to 5 (full), or 0 when the client does not report one.
    pub fn battery_status(&self) -> u8 {
        match self.battery {
            (3, _) => 0xEE,
            (5, _) => 0xEF,
            (_, 0xFF) | (0 | 1, _) => 0,
            (_, percent) => match percent {
                0..=10 => 1,
                11..=40 => 2,
                41..=70 => 3,
                71..=99 => 4,
                _ => 5,
            },
        }
    }
}

/// The four slots: which session's controller holds each, and its state.
#[derive(Debug, Default)]
pub struct Slots {
    owners: [Option<(String, u8)>; SLOTS],
    pads: [Pad; SLOTS],
    numbers: [u32; SLOTS],
}

impl Slots {
    /// The slot of `session`'s controller `id`.
    fn find(&self, session: &str, id: u8) -> Option<usize> {
        self.owners
            .iter()
            .position(|o| o.as_ref().is_some_and(|(s, i)| s == session && *i == id))
    }

    /// Applies an input of `session`; returns the slot that changed. A
    /// controller takes the first free slot when it arrives or first sends
    /// state, and leaves its slot when its client drops it from the active
    /// mask; one beyond the four gets none.
    pub fn apply(&mut self, session: &str, input: &Input, motion_us: u64) -> Option<u8> {
        let id = match *input {
            Input::Controller { id, active, .. } => {
                let id = u8::try_from(id).ok()?;
                if id >= 16 || active & (1 << id) == 0 {
                    let slot = self.find(session, id)?;
                    self.owners[slot] = None;
                    self.pads[slot] = Pad::default();
                    return Some(slot as u8);
                }
                id
            }
            Input::Arrival { id, .. }
            | Input::ControllerTouch { id, .. }
            | Input::Motion { id, .. }
            | Input::Battery { id, .. } => id,
            _ => return None,
        };
        let slot = match self.find(session, id) {
            Some(slot) => slot,
            None => {
                let slot = self.owners.iter().position(Option::is_none)?;
                self.owners[slot] = Some((session.to_owned(), id));
                self.pads[slot] = Pad::default();
                if !matches!(input, Input::Arrival { .. }) {
                    self.pads[slot].apply(input, motion_us);
                }
                return Some(slot as u8);
            }
        };
        self.pads[slot]
            .apply(input, motion_us)
            .then_some(slot as u8)
    }

    /// Frees every slot of a session that ended; returns them.
    pub fn forget(&mut self, session: &str) -> Vec<u8> {
        let mut freed = Vec::new();
        for slot in 0..SLOTS {
            if self.owners[slot]
                .as_ref()
                .is_some_and(|(s, _)| s == session)
            {
                self.owners[slot] = None;
                self.pads[slot] = Pad::default();
                freed.push(slot as u8);
            }
        }
        freed
    }

    /// A connected slot's state.
    pub fn pad(&self, slot: u8) -> Option<&Pad> {
        let slot = usize::from(slot);
        self.owners.get(slot)?.as_ref()?;
        self.pads.get(slot)
    }

    /// The next packet number of a slot.
    pub fn next_number(&mut self, slot: u8) -> u32 {
        let number = &mut self.numbers[usize::from(slot) % SLOTS];
        *number = number.wrapping_add(1);
        *number
    }
}

/// When a client last asked for each kind of pad data.
#[derive(Debug, Default)]
struct Registration {
    all: Option<Instant>,
    slots: [Option<Instant>; SLOTS],
    macs: Vec<([u8; 6], Instant)>,
}

/// The clients that asked for pad data, by address, as DS4Windows keeps them.
#[derive(Debug, Default)]
pub struct Clients {
    clients: HashMap<SocketAddr, Registration>,
}

impl Clients {
    /// Records a [`Request::PadData`] from `from`.
    pub fn register(&mut self, from: SocketAddr, flags: u8, slot: u8, mac: [u8; 6], now: Instant) {
        let client = self.clients.entry(from).or_default();
        if flags == 0 {
            client.all = Some(now);
        }
        if flags & 1 != 0 && usize::from(slot) < SLOTS {
            client.slots[usize::from(slot)] = Some(now);
        }
        if flags & 2 != 0 {
            match client.macs.iter_mut().find(|(m, _)| *m == mac) {
                Some((_, at)) => *at = now,
                None => client.macs.push((mac, now)),
            }
        }
    }

    /// The clients that want `slot`'s pad data; drops those that stopped asking.
    pub fn subscribers(&mut self, slot: u8, now: Instant) -> Vec<SocketAddr> {
        let fresh =
            |at: &Option<Instant>| at.is_some_and(|at| now.duration_since(at) < CLIENT_TIMEOUT);
        self.prune(now);
        let address = mac(slot);
        self.clients
            .iter()
            .filter(|(_, c)| {
                fresh(&c.all)
                    || c.slots.get(usize::from(slot)).is_some_and(fresh)
                    || c.macs
                        .iter()
                        .any(|(m, at)| *m == address && fresh(&Some(*at)))
            })
            .map(|(address, _)| *address)
            .collect()
    }

    /// Drops clients whose every registration timed out.
    pub fn prune(&mut self, now: Instant) {
        let fresh = |at: &Instant| now.duration_since(*at) < CLIENT_TIMEOUT;
        self.clients.retain(|_, c| {
            c.macs.retain(|(_, at)| fresh(at));
            c.all.as_ref().is_some_and(fresh)
                || c.slots.iter().flatten().any(fresh)
                || !c.macs.is_empty()
        });
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

#[cfg(test)]
mod tests;
