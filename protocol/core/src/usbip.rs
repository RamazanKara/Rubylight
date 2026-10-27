//! USB/IP framing for USB forwarding: after the USB tunnel's upgrade, the client serves the
//! device as a USB/IP server and the host relays it to its USB/IP driver. This is the Linux
//! kernel's protocol, version 1.1.1, big-endian.
//!
//! The driver opens with `OP_REQ_IMPORT` (an 8-byte op header and the 32-byte busid); the server
//! answers `OP_REP_IMPORT`, with the 312-byte [`Device`] when its status is 0. Then every
//! message is a 48-byte [`Header`]: a `CMD_SUBMIT` carries its OUT data and then 16 bytes per
//! isochronous packet, a `RET_SUBMIT` its IN data (`actual_length` bytes) and then the packets.
//! A `RET_SUBMIT` does not repeat the direction: both sides know it from the command's seqnum.

use alloc::string::String;

/// USB/IP version 1.1.1.
pub const VERSION: u16 = 0x0111;
pub const OP_REQ_IMPORT: u16 = 0x8003;
pub const OP_REP_IMPORT: u16 = 0x0003;
/// version u16, code u16, status u32.
pub const OP_HEADER_BYTES: usize = 8;
pub const BUSID_BYTES: usize = 32;
pub const PATH_BYTES: usize = 256;
/// An `OP_REQ_IMPORT`: the op header and the busid.
pub const IMPORT_REQUEST_BYTES: usize = OP_HEADER_BYTES + BUSID_BYTES;
/// An encoded [`Device`] (`usbip_usb_device`).
pub const DEVICE_BYTES: usize = 312;
/// Every message after the import.
pub const HEADER_BYTES: usize = 48;
/// One isochronous packet descriptor.
pub const ISO_PACKET_BYTES: usize = 16;

pub const CMD_SUBMIT: u32 = 1;
pub const CMD_UNLINK: u32 = 2;
pub const RET_SUBMIT: u32 = 3;
pub const RET_UNLINK: u32 = 4;

pub const DIR_OUT: u32 = 0;
pub const DIR_IN: u32 = 1;

/// Linux errno values, negated, as the statuses carry them.
pub const ENOENT: i32 = -2;
pub const ENOMEM: i32 = -12;
pub const ENODEV: i32 = -19;
pub const EINVAL: i32 = -22;
pub const EPIPE: i32 = -32;
pub const EPROTO: i32 = -71;
pub const EOVERFLOW: i32 = -75;
pub const ECONNRESET: i32 = -104;
pub const ESHUTDOWN: i32 = -108;
pub const ETIMEDOUT: i32 = -110;

/// The largest transfer either side accepts. Drivers split bigger ones; a mass-storage read
/// is at most 1 MiB on Windows.
pub const MAX_TRANSFER: u32 = 4 << 20;
/// The most isochronous packets one transfer carries.
pub const MAX_ISO_PACKETS: u32 = 1024;
/// The number_of_packets some stacks send for a transfer that is not isochronous.
pub const NOT_ISO: u32 = 0xffff_ffff;

/// The 8-byte header of `OP_REQ_IMPORT` and `OP_REP_IMPORT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpHeader {
    pub version: u16,
    pub code: u16,
    pub status: u32,
}

impl OpHeader {
    pub fn encode(self) -> [u8; OP_HEADER_BYTES] {
        let mut out = [0; OP_HEADER_BYTES];
        out[0..2].copy_from_slice(&self.version.to_be_bytes());
        out[2..4].copy_from_slice(&self.code.to_be_bytes());
        out[4..8].copy_from_slice(&self.status.to_be_bytes());
        out
    }

    pub fn decode(data: &[u8; OP_HEADER_BYTES]) -> Self {
        Self {
            version: u16::from_be_bytes([data[0], data[1]]),
            code: u16::from_be_bytes([data[2], data[3]]),
            status: be32(data, 4),
        }
    }
}

/// An `OP_REQ_IMPORT` for `busid`; None when it does not fit.
pub fn import_request(busid: &str) -> Option<[u8; IMPORT_REQUEST_BYTES]> {
    let mut out = [0; IMPORT_REQUEST_BYTES];
    out[..OP_HEADER_BYTES].copy_from_slice(&OpHeader { version: VERSION, code: OP_REQ_IMPORT, status: 0 }.encode());
    put_text(&mut out[OP_HEADER_BYTES..], busid)?;
    Some(out)
}

/// The busid an `OP_REQ_IMPORT` asks for; None for another op or a busid that is not text.
pub fn decode_import_request(data: &[u8; IMPORT_REQUEST_BYTES]) -> Option<String> {
    let header = OpHeader::decode(data[..OP_HEADER_BYTES].try_into().ok()?);
    if header.code != OP_REQ_IMPORT {
        return None;
    }
    text(&data[OP_HEADER_BYTES..])
}

/// The op header of an `OP_REP_IMPORT`; a [`Device`] follows when `status` is 0.
pub fn import_reply(status: u32) -> [u8; OP_HEADER_BYTES] {
    OpHeader { version: VERSION, code: OP_REP_IMPORT, status }.encode()
}

/// `usbip_usb_device`: the device as the import reply describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub path: String,
    pub busid: String,
    pub busnum: u32,
    pub devnum: u32,
    /// 1 low, 2 full, 3 high, 4 wireless, 5 super, 6 super+.
    pub speed: u32,
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub configuration: u8,
    pub configurations: u8,
    pub interfaces: u8,
}

impl Device {
    /// None when the path or busid does not fit.
    pub fn encode(&self) -> Option<[u8; DEVICE_BYTES]> {
        let mut out = [0; DEVICE_BYTES];
        put_text(&mut out[..PATH_BYTES], &self.path)?;
        put_text(&mut out[PATH_BYTES..PATH_BYTES + BUSID_BYTES], &self.busid)?;
        let mut at = PATH_BYTES + BUSID_BYTES;
        for value in [self.busnum, self.devnum, self.speed] {
            out[at..at + 4].copy_from_slice(&value.to_be_bytes());
            at += 4;
        }
        for value in [self.vendor, self.product, self.bcd_device] {
            out[at..at + 2].copy_from_slice(&value.to_be_bytes());
            at += 2;
        }
        out[at..].copy_from_slice(&[
            self.class,
            self.subclass,
            self.protocol,
            self.configuration,
            self.configurations,
            self.interfaces,
        ]);
        Some(out)
    }

    pub fn decode(data: &[u8; DEVICE_BYTES]) -> Option<Self> {
        let at = PATH_BYTES + BUSID_BYTES;
        let be16 = |at: usize| u16::from_be_bytes([data[at], data[at + 1]]);
        Some(Self {
            path: text(&data[..PATH_BYTES])?,
            busid: text(&data[PATH_BYTES..at])?,
            busnum: be32(data, at),
            devnum: be32(data, at + 4),
            speed: be32(data, at + 8),
            vendor: be16(at + 12),
            product: be16(at + 14),
            bcd_device: be16(at + 16),
            class: data[at + 18],
            subclass: data[at + 19],
            protocol: data[at + 20],
            configuration: data[at + 21],
            configurations: data[at + 22],
            interfaces: data[at + 23],
        })
    }
}

/// The fields every [`Header`] starts with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Base {
    pub seqnum: u32,
    pub devid: u32,
    /// [`DIR_OUT`] or [`DIR_IN`]; 0 in replies.
    pub direction: u32,
    /// The endpoint number, 0-15; 0 in replies.
    pub ep: u32,
}

/// One 48-byte message after the import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    Submit {
        base: Base,
        flags: u32,
        /// The OUT data that follows, or the IN buffer's size.
        length: u32,
        start_frame: i32,
        /// Isochronous packets, 0 or [`NOT_ISO`] otherwise.
        packets: u32,
        interval: u32,
        setup: [u8; 8],
    },
    Unlink {
        base: Base,
        /// The seqnum of the submit to cancel.
        unlink: u32,
    },
    RetSubmit {
        base: Base,
        status: i32,
        /// The bytes transferred; IN data of this size follows.
        length: u32,
        start_frame: i32,
        packets: u32,
        errors: u32,
    },
    RetUnlink {
        base: Base,
        /// [`ECONNRESET`] when the submit was cancelled, 0 when it had already completed.
        status: i32,
    },
}

impl Header {
    pub fn base(&self) -> Base {
        match *self {
            Self::Submit { base, .. }
            | Self::Unlink { base, .. }
            | Self::RetSubmit { base, .. }
            | Self::RetUnlink { base, .. } => base,
        }
    }

    pub fn encode(&self) -> [u8; HEADER_BYTES] {
        let mut out = [0; HEADER_BYTES];
        let (command, base, words): (u32, Base, [u32; 5]) = match *self {
            Self::Submit { base, flags, length, start_frame, packets, interval, setup } => {
                out[40..48].copy_from_slice(&setup);
                (CMD_SUBMIT, base, [flags, length, start_frame as u32, packets, interval])
            }
            Self::Unlink { base, unlink } => (CMD_UNLINK, base, [unlink, 0, 0, 0, 0]),
            Self::RetSubmit { base, status, length, start_frame, packets, errors } => {
                (RET_SUBMIT, base, [status as u32, length, start_frame as u32, packets, errors])
            }
            Self::RetUnlink { base, status } => (RET_UNLINK, base, [status as u32, 0, 0, 0, 0]),
        };
        for (at, value) in [command, base.seqnum, base.devid, base.direction, base.ep]
            .into_iter()
            .chain(words)
            .enumerate()
        {
            out[at * 4..at * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
        out
    }

    /// None for an unknown command.
    pub fn decode(data: &[u8; HEADER_BYTES]) -> Option<Self> {
        let word = |index: usize| be32(data, index * 4);
        let base = Base { seqnum: word(1), devid: word(2), direction: word(3), ep: word(4) };
        Some(match word(0) {
            CMD_SUBMIT => Self::Submit {
                base,
                flags: word(5),
                length: word(6),
                start_frame: word(7) as i32,
                packets: word(8),
                interval: word(9),
                setup: data[40..48].try_into().ok()?,
            },
            CMD_UNLINK => Self::Unlink { base, unlink: word(5) },
            RET_SUBMIT => Self::RetSubmit {
                base,
                status: word(5) as i32,
                length: word(6),
                start_frame: word(7) as i32,
                packets: word(8),
                errors: word(9),
            },
            RET_UNLINK => Self::RetUnlink { base, status: word(5) as i32 },
            _ => return None,
        })
    }
}

/// The isochronous packets a transfer carries: 0 for the "not isochronous" marker.
pub fn iso_packets(packets: u32) -> u32 {
    if packets == NOT_ISO { 0 } else { packets }
}

/// How many bytes follow a `CMD_SUBMIT`: its OUT data and its packet descriptors. None when it
/// asks for more than [`MAX_TRANSFER`] or [`MAX_ISO_PACKETS`].
pub fn submit_payload(direction: u32, length: u32, packets: u32) -> Option<usize> {
    let packets = iso_packets(packets);
    if length > MAX_TRANSFER || packets > MAX_ISO_PACKETS {
        return None;
    }
    let data = if direction == DIR_OUT { length as usize } else { 0 };
    Some(data + packets as usize * ISO_PACKET_BYTES)
}

/// How many bytes follow a `RET_SUBMIT` for a submit of `direction`: the IN data and the packet
/// descriptors. None when the reply does not fit the submit: more data than it asked for, or
/// other packets (the driver reads as many descriptors as it sent).
pub fn reply_payload(direction: u32, asked: u32, asked_packets: u32, length: u32, packets: u32) -> Option<usize> {
    let (asked_packets, packets) = (iso_packets(asked_packets), iso_packets(packets));
    if length > asked || packets != asked_packets {
        return None;
    }
    let data = if direction == DIR_IN { length as usize } else { 0 };
    Some(data + packets as usize * ISO_PACKET_BYTES)
}

/// One isochronous packet descriptor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IsoPacket {
    pub offset: u32,
    pub length: u32,
    pub actual_length: u32,
    pub status: i32,
}

impl IsoPacket {
    pub fn encode(&self) -> [u8; ISO_PACKET_BYTES] {
        let mut out = [0; ISO_PACKET_BYTES];
        for (at, value) in [self.offset, self.length, self.actual_length, self.status as u32].into_iter().enumerate() {
            out[at * 4..at * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
        out
    }

    pub fn decode(data: &[u8; ISO_PACKET_BYTES]) -> Self {
        Self {
            offset: be32(data, 0),
            length: be32(data, 4),
            actual_length: be32(data, 8),
            status: be32(data, 12) as i32,
        }
    }
}

/// The packet descriptors that end a payload carrying `packets` of them; none for the "not
/// isochronous" marker. None when the payload is too short to hold them.
pub fn iso_descriptors(payload: &[u8], packets: u32) -> Option<impl Iterator<Item = IsoPacket> + '_> {
    let start = payload.len().checked_sub(iso_packets(packets) as usize * ISO_PACKET_BYTES)?;
    Some(payload[start..].chunks_exact(ISO_PACKET_BYTES).map(|chunk| {
        let mut raw = [0; ISO_PACKET_BYTES];
        raw.copy_from_slice(chunk);
        IsoPacket::decode(&raw)
    }))
}

/// Whether a `RET_SUBMIT`'s packets fit the submit's: as many, the same offsets and lengths, no
/// packet longer than its slot, and actual lengths that add up to the reply's `length` (IN data
/// comes packed; Linux checks the sum both ways).
pub fn iso_reply_fits(asked: &[IsoPacket], replied: impl Iterator<Item = IsoPacket>, length: u32) -> bool {
    let (mut count, mut total) = (0, 0u64);
    for replied in replied {
        let Some(asked) = asked.get(count) else {
            return false;
        };
        if replied.offset != asked.offset || replied.length != asked.length || replied.actual_length > asked.length {
            return false;
        }
        total += u64::from(replied.actual_length);
        count += 1;
    }
    count == asked.len() && total == u64::from(length)
}

/// A USB/IP busid either side accepts: 1-31 bytes of letters, digits, `-`, `.`, `:` and `_`,
/// not starting with `-` (it reaches the driver's command line).
pub fn valid_busid(busid: &str) -> bool {
    (1..BUSID_BYTES).contains(&busid.len())
        && !busid.starts_with('-')
        && busid.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':' | b'_'))
}

fn be32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// A NUL-padded field; the text must leave room for one NUL.
fn put_text(field: &mut [u8], value: &str) -> Option<()> {
    if value.len() >= field.len() || value.as_bytes().contains(&0) {
        return None;
    }
    field[..value.len()].copy_from_slice(value.as_bytes());
    Some(())
}

fn text(field: &[u8]) -> Option<String> {
    let end = field.iter().position(|&b| b == 0)?;
    String::from_utf8(field[..end].into()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_request_round_trips() {
        let request = import_request("1-2.3").unwrap();
        assert_eq!(&request[..8], &[0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0]);
        assert_eq!(decode_import_request(&request).as_deref(), Some("1-2.3"));
        assert!(import_request(&"1".repeat(32)).is_none());
        let mut other = request;
        other[3] = 0x05;
        assert_eq!(decode_import_request(&other), None);
    }

    #[test]
    fn device_matches_the_kernel_layout() {
        let device = Device {
            path: "/sys/devices/usb1/1-1".into(),
            busid: "1-1".into(),
            busnum: 1,
            devnum: 2,
            speed: 3,
            vendor: 0x1234,
            product: 0x5678,
            bcd_device: 0x0100,
            class: 8,
            subclass: 6,
            protocol: 0x50,
            configuration: 1,
            configurations: 1,
            interfaces: 1,
        };
        let bytes = device.encode().unwrap();
        assert_eq!(&bytes[288..300], &[0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3]);
        assert_eq!(&bytes[300..306], &[0x12, 0x34, 0x56, 0x78, 0x01, 0x00]);
        assert_eq!(&bytes[306..], &[8, 6, 0x50, 1, 1, 1]);
        assert_eq!(Device::decode(&bytes), Some(device));
        let mut unterminated = bytes;
        unterminated[256..288].fill(b'1');
        assert_eq!(Device::decode(&unterminated), None);
    }

    #[test]
    fn headers_round_trip_in_kernel_order() {
        let base = Base { seqnum: 7, devid: 0x0001_0002, direction: DIR_IN, ep: 1 };
        let submit = Header::Submit {
            base,
            flags: 0x200,
            length: 512,
            start_frame: -1,
            packets: NOT_ISO,
            interval: 4,
            setup: [0x80, 6, 0, 1, 0, 0, 18, 0],
        };
        let bytes = submit.encode();
        assert_eq!(&bytes[..4], &[0, 0, 0, 1]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 7]);
        assert_eq!(&bytes[24..28], &[0, 0, 2, 0]);
        assert_eq!(&bytes[40..], &[0x80, 6, 0, 1, 0, 0, 18, 0]);
        assert_eq!(Header::decode(&bytes), Some(submit));
        for header in [
            Header::Unlink { base, unlink: 6 },
            Header::RetSubmit { base: Base { seqnum: 7, ..Base::default() }, status: EPIPE, length: 0, start_frame: 0, packets: 0, errors: 0 },
            Header::RetUnlink { base: Base { seqnum: 8, ..Base::default() }, status: ECONNRESET },
        ] {
            assert_eq!(Header::decode(&header.encode()), Some(header));
        }
        let mut unknown = bytes;
        unknown[3] = 5;
        assert_eq!(Header::decode(&unknown), None);
    }

    #[test]
    fn payloads_follow_the_direction_and_stay_bounded() {
        assert_eq!(submit_payload(DIR_OUT, 512, 0), Some(512));
        assert_eq!(submit_payload(DIR_IN, 512, NOT_ISO), Some(0));
        assert_eq!(submit_payload(DIR_IN, 192, 3), Some(48));
        assert_eq!(submit_payload(DIR_OUT, MAX_TRANSFER + 1, 0), None);
        assert_eq!(submit_payload(DIR_IN, 0, MAX_ISO_PACKETS + 1), None);
        assert_eq!(reply_payload(DIR_IN, 512, 0, 18, 0), Some(18));
        assert_eq!(reply_payload(DIR_OUT, 512, 0, 512, 0), Some(0));
        assert_eq!(reply_payload(DIR_IN, 192, 3, 150, 3), Some(150 + 48));
        assert_eq!(reply_payload(DIR_IN, 512, 0, 513, 0), None, "more than asked for");
        assert_eq!(reply_payload(DIR_IN, 192, 3, 0, 4), None, "other packets");
        assert_eq!(reply_payload(DIR_IN, 192, 3, 0, 0), None, "no packets");
    }

    #[test]
    fn iso_packets_round_trip() {
        let packet = IsoPacket { offset: 64, length: 64, actual_length: 60, status: EOVERFLOW };
        assert_eq!(IsoPacket::decode(&packet.encode()), packet);
    }

    #[test]
    fn iso_descriptors_come_from_the_payload_tail() {
        let packets = [
            IsoPacket { offset: 0, length: 64, actual_length: 60, status: 0 },
            IsoPacket { offset: 64, length: 64, actual_length: 64, status: 0 },
        ];
        let mut payload = alloc::vec![0xaa; 124];
        payload.extend(packets.iter().flat_map(|packet| packet.encode()));
        assert!(iso_descriptors(&payload, 2).unwrap().eq(packets));
        assert_eq!(iso_descriptors(&payload, NOT_ISO).unwrap().count(), 0);
        assert!(iso_descriptors(&payload[..20], 2).is_none());
    }

    #[test]
    fn iso_replies_keep_the_submitted_layout_and_add_up() {
        let asked = [
            IsoPacket { offset: 0, length: 64, actual_length: 0, status: 0 },
            IsoPacket { offset: 64, length: 64, actual_length: 0, status: 0 },
        ];
        let reply = |first: IsoPacket, second: IsoPacket| [first, second].into_iter();
        let good = IsoPacket { actual_length: 60, ..asked[0] };
        let full = IsoPacket { actual_length: 64, ..asked[1] };
        assert!(iso_reply_fits(&asked, reply(good, full), 124));
        assert!(!iso_reply_fits(&asked, reply(good, full), 128), "the sum differs from the length");
        assert!(!iso_reply_fits(&asked, reply(IsoPacket { offset: 4, ..good }, full), 124), "moved");
        assert!(!iso_reply_fits(&asked, reply(IsoPacket { length: 128, ..good }, full), 124), "resized");
        let long = IsoPacket { actual_length: 65, ..full };
        assert!(!iso_reply_fits(&asked, reply(IsoPacket { actual_length: 59, ..good }, long), 124), "longer than its slot");
        assert!(!iso_reply_fits(&asked, [good].into_iter(), 60), "fewer packets");
        assert!(!iso_reply_fits(&asked[..1], reply(good, full), 124), "more packets");
    }

    #[test]
    fn busids_stay_safe_for_the_driver() {
        assert!(valid_busid("1-1"));
        assert!(valid_busid("3-2.1:1.0"));
        assert!(!valid_busid(""));
        assert!(!valid_busid("-1"));
        assert!(!valid_busid("1 1"));
        assert!(!valid_busid(&"1".repeat(32)));
    }
}
