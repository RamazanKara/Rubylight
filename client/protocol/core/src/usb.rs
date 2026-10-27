//! USB forwarding: the control message (0x553A, both directions) that attaches a device
//! plugged into the client to the host.
//!
//! The client asks with [`UsbMessage::AttachRequest`]; the host answers with a
//! [`UsbMessage::TunnelReady`] token for the native API's USB tunnel, then a
//! [`UsbMessage::Status`]. Either side ends an attachment with [`UsbMessage::Detach`]. After the
//! tunnel's upgrade its bytes are standard USB/IP with the client as the USB/IP server. Every
//! message starts with the same 8-byte header, little-endian:
//!
//! | Off | Type | Field |
//! |---|---|---|
//! | 0 | u8 | version, 1 |
//! | 1 | u8 | kind: 0 attach request, 1 tunnel ready, 2 detach, 3 status |
//! | 2 | u16 | reserved, 0 |
//! | 4 | u32 | request_id: picked by the client, non-zero, names the attachment |
//!
//! An attach request goes on with vid u16, pid u16, class u8, speed u8 (as USB/IP numbers
//! them), busid_len u8 (1-31), name_len u8 (0-64), the ASCII USB/IP busid and the UTF-8
//! name. A tunnel ready carries the 16-byte token at 8; a status carries state u8 at 8,
//! reason u8 at 9 and a reserved u16.

use alloc::string::String;
use alloc::vec::Vec;

/// Control-stream message type of a [`UsbMessage`] (both directions).
pub const USB_MESSAGE_TYPE: u16 = 0x553A;
/// Size of the header every [`UsbMessage`] starts with, and of a whole detach.
pub const USB_HEADER_BYTES: usize = 8;
/// Size of an attach request before its busid and name.
pub const USB_ATTACH_REQUEST_BYTES: usize = 16;
/// Encoded size of a tunnel ready.
pub const USB_TUNNEL_READY_BYTES: usize = 24;
/// Encoded size of a status.
pub const USB_STATUS_BYTES: usize = 12;
/// Longest USB/IP busid, in bytes.
pub const MAX_BUSID_BYTES: usize = 31;
/// Longest device name, in bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 64;
/// Size of a tunnel token.
pub const TOKEN_BYTES: usize = 16;

const VERSION: u8 = 1;
const ATTACH_REQUEST: u8 = 0;
const TUNNEL_READY: u8 = 1;
const DETACH: u8 = 2;
const STATUS: u8 = 3;

/// Where an attachment stands, as the host reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbState {
    Attached,
    Detached,
    Refused,
    Failed,
}

impl UsbState {
    fn wire(self) -> u8 {
        match self {
            Self::Attached => 0,
            Self::Detached => 1,
            Self::Refused => 2,
            Self::Failed => 3,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Attached),
            1 => Some(Self::Detached),
            2 => Some(Self::Refused),
            3 => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Why the host refused, failed or detached an attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbReason {
    None,
    /// The device has no USB permission on this host.
    Permission,
    /// USB forwarding is off on this host.
    Disabled,
    /// The host has as many devices attached as it allows.
    Limit,
    /// The host's USB/IP driver is missing or failed.
    Driver,
    /// The client did not open the tunnel within 30 s.
    TunnelTimeout,
    /// The device stopped answering.
    DeviceError,
    /// The stream ended.
    SessionEnd,
}

impl UsbReason {
    fn wire(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Permission => 1,
            Self::Disabled => 2,
            Self::Limit => 3,
            Self::Driver => 4,
            Self::TunnelTimeout => 5,
            Self::DeviceError => 6,
            Self::SessionEnd => 7,
        }
    }

    fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::None),
            1 => Some(Self::Permission),
            2 => Some(Self::Disabled),
            3 => Some(Self::Limit),
            4 => Some(Self::Driver),
            5 => Some(Self::TunnelTimeout),
            6 => Some(Self::DeviceError),
            7 => Some(Self::SessionEnd),
            _ => None,
        }
    }
}

/// One USB forwarding message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsbMessage {
    /// Client to host: attach this device.
    AttachRequest {
        request_id: u32,
        vid: u16,
        pid: u16,
        /// bDeviceClass, or the first interface's class when that is 0.
        class: u8,
        /// USB/IP's numbering: 1 low, 2 full, 3 high, 4 wireless, 5 super, 6 super+.
        speed: u8,
        /// The USB/IP busid, 1-31 printable ASCII bytes.
        busid: String,
        /// What the user sees, at most 64 bytes.
        name: String,
    },
    /// Host to client: open the tunnel with this token within 30 s.
    TunnelReady {
        request_id: u32,
        token: [u8; TOKEN_BYTES],
    },
    /// Either way: the attachment ends.
    Detach { request_id: u32 },
    /// Host to client: what became of the attachment.
    Status {
        request_id: u32,
        state: UsbState,
        reason: UsbReason,
    },
}

impl UsbMessage {
    pub fn request_id(&self) -> u32 {
        match self {
            Self::AttachRequest { request_id, .. }
            | Self::TunnelReady { request_id, .. }
            | Self::Detach { request_id }
            | Self::Status { request_id, .. } => *request_id,
        }
    }

    /// The wire form, or `None` for a zero request id or a busid or name out of bounds.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.request_id() == 0 {
            return None;
        }
        let kind = match self {
            Self::AttachRequest { .. } => ATTACH_REQUEST,
            Self::TunnelReady { .. } => TUNNEL_READY,
            Self::Detach { .. } => DETACH,
            Self::Status { .. } => STATUS,
        };
        let mut out = Vec::with_capacity(USB_TUNNEL_READY_BYTES);
        out.extend_from_slice(&[VERSION, kind, 0, 0]);
        out.extend_from_slice(&self.request_id().to_le_bytes());
        match self {
            Self::AttachRequest {
                vid,
                pid,
                class,
                speed,
                busid,
                name,
                ..
            } => {
                if !busid_valid(busid.as_bytes()) || name.len() > MAX_NAME_BYTES {
                    return None;
                }
                out.extend_from_slice(&vid.to_le_bytes());
                out.extend_from_slice(&pid.to_le_bytes());
                out.extend_from_slice(&[*class, *speed, busid.len() as u8, name.len() as u8]);
                out.extend_from_slice(busid.as_bytes());
                out.extend_from_slice(name.as_bytes());
            }
            Self::TunnelReady { token, .. } => out.extend_from_slice(token),
            Self::Detach { .. } => {}
            Self::Status { state, reason, .. } => {
                out.extend_from_slice(&[state.wire(), reason.wire(), 0, 0]);
            }
        }
        Some(out)
    }

    /// Parses a message. Version 0, a zero request id, an unknown kind, state or reason, a
    /// busid that is empty, longer than 31 bytes or not printable ASCII, a name longer than
    /// 64 bytes or not UTF-8, and a short message reject it. Bytes past the known fields are
    /// ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < USB_HEADER_BYTES || data[0] == 0 {
            return None;
        }
        let request_id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if request_id == 0 {
            return None;
        }
        match data[1] {
            ATTACH_REQUEST => {
                if data.len() < USB_ATTACH_REQUEST_BYTES {
                    return None;
                }
                let (busid_len, name_len) = (usize::from(data[14]), usize::from(data[15]));
                if name_len > MAX_NAME_BYTES {
                    return None;
                }
                let busid_end = USB_ATTACH_REQUEST_BYTES + busid_len;
                let busid = data.get(USB_ATTACH_REQUEST_BYTES..busid_end)?;
                let name = data.get(busid_end..busid_end + name_len)?;
                if !busid_valid(busid) {
                    return None;
                }
                Some(Self::AttachRequest {
                    request_id,
                    vid: u16::from_le_bytes([data[8], data[9]]),
                    pid: u16::from_le_bytes([data[10], data[11]]),
                    class: data[12],
                    speed: data[13],
                    busid: String::from_utf8(busid.to_vec()).ok()?,
                    name: String::from_utf8(name.to_vec()).ok()?,
                })
            }
            TUNNEL_READY => Some(Self::TunnelReady {
                request_id,
                token: data.get(8..USB_TUNNEL_READY_BYTES)?.try_into().ok()?,
            }),
            DETACH => Some(Self::Detach { request_id }),
            STATUS => {
                if data.len() < USB_STATUS_BYTES {
                    return None;
                }
                Some(Self::Status {
                    request_id,
                    state: UsbState::from_wire(data[8])?,
                    reason: UsbReason::from_wire(data[9])?,
                })
            }
            _ => None,
        }
    }
}

fn busid_valid(busid: &[u8]) -> bool {
    (1..=MAX_BUSID_BYTES).contains(&busid.len()) && busid.iter().all(u8::is_ascii_graphic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attach() -> UsbMessage {
        UsbMessage::AttachRequest {
            request_id: 3,
            vid: 0x054c,
            pid: 0x0ce6,
            class: 0,
            speed: 3,
            busid: "1-1.2".into(),
            name: "Controller Ä".into(),
        }
    }

    #[test]
    fn every_kind_round_trips() {
        let messages = [
            attach(),
            UsbMessage::TunnelReady {
                request_id: 3,
                token: [0xA5; TOKEN_BYTES],
            },
            UsbMessage::Detach { request_id: 3 },
            UsbMessage::Status {
                request_id: 3,
                state: UsbState::Refused,
                reason: UsbReason::Limit,
            },
        ];
        let sizes = [USB_ATTACH_REQUEST_BYTES + 5 + 13, 24, 8, 12];
        for (message, size) in messages.iter().zip(sizes) {
            let wire = message.encode().unwrap();
            assert_eq!(wire.len(), size, "{message:?}");
            assert_eq!(wire[0], 1);
            assert_eq!(wire[4..8], 3u32.to_le_bytes());
            assert_eq!(UsbMessage::decode(&wire).as_ref(), Some(message));
            // A newer version's tail is ignored.
            let mut longer = wire.clone();
            longer[0] = 2;
            longer.extend_from_slice(&[0xEE; 6]);
            assert_eq!(UsbMessage::decode(&longer).as_ref(), Some(message));
            assert_eq!(UsbMessage::decode(&wire[..size - 1]), None, "{message:?}");
        }
        let wire = attach().encode().unwrap();
        assert_eq!(wire[8..16], [0x4c, 0x05, 0xe6, 0x0c, 0, 3, 5, 13]);
        assert_eq!(&wire[16..21], b"1-1.2");
    }

    #[test]
    fn every_state_and_reason_round_trips() {
        let states = [
            UsbState::Attached,
            UsbState::Detached,
            UsbState::Refused,
            UsbState::Failed,
        ];
        for (wire, state) in states.into_iter().enumerate() {
            assert_eq!(state.wire(), wire as u8);
            assert_eq!(UsbState::from_wire(wire as u8), Some(state));
        }
        assert_eq!(UsbState::from_wire(4), None);
        let reasons = [
            UsbReason::None,
            UsbReason::Permission,
            UsbReason::Disabled,
            UsbReason::Limit,
            UsbReason::Driver,
            UsbReason::TunnelTimeout,
            UsbReason::DeviceError,
            UsbReason::SessionEnd,
        ];
        for (wire, reason) in reasons.into_iter().enumerate() {
            assert_eq!(reason.wire(), wire as u8);
            assert_eq!(UsbReason::from_wire(wire as u8), Some(reason));
        }
        assert_eq!(UsbReason::from_wire(8), None);
    }

    #[test]
    fn bad_messages_are_refused() {
        let with = |busid: &str, name: &str| UsbMessage::AttachRequest {
            request_id: 1,
            vid: 1,
            pid: 2,
            class: 3,
            speed: 2,
            busid: busid.into(),
            name: name.into(),
        };
        assert!(with("1-1", "").encode().is_some());
        assert!(with(&"1".repeat(31), &"n".repeat(64)).encode().is_some());
        for message in [
            with("", "x"),
            with(&"1".repeat(32), "x"),
            with("1 1", "x"),
            with("1-ä", "x"),
            with("1-1", &"n".repeat(65)),
            UsbMessage::Detach { request_id: 0 },
        ] {
            assert_eq!(message.encode(), None, "{message:?}");
        }

        let wire = attach().encode().unwrap();
        let patched = |at: usize, byte: u8| {
            let mut wire = wire.clone();
            wire[at] = byte;
            UsbMessage::decode(&wire)
        };
        assert_eq!(patched(0, 0), None, "version 0");
        assert_eq!(patched(1, 4), None, "unknown kind");
        assert_eq!(patched(4, 0), None, "request id 0");
        assert_eq!(patched(14, 0), None, "empty busid");
        assert_eq!(patched(14, 32), None, "busid past 31 bytes");
        assert_eq!(patched(15, 65), None, "name past 64 bytes");
        assert_eq!(patched(16, b' '), None, "busid not printable");
        assert_eq!(patched(16 + 5 + 11, 0xFF), None, "name not UTF-8");

        let status = UsbMessage::Status {
            request_id: 9,
            state: UsbState::Attached,
            reason: UsbReason::None,
        };
        let mut wire = status.encode().unwrap();
        wire[8] = 4;
        assert_eq!(UsbMessage::decode(&wire), None, "unknown state");
        wire[8] = 0;
        wire[9] = 8;
        assert_eq!(UsbMessage::decode(&wire), None, "unknown reason");
    }
}
