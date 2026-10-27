//! Steam Controller (2026) extensions to the Moonlight protocol, as Vibepollo and the
//! jlobue10 moonlight-common-c fork define them (Vibepollo `src/steam_controller_protocol.h`).
//!
//! A client announces a Steam Controller with [`LI_CTYPE_STEAM`] and the capability bits
//! below in its arrival event. The host sends haptic reports ([`SteamHaptic`], 0x5504) and
//! settings writes ([`SteamSettings`], 0x5505) on the encrypted control stream only to a
//! client that advertised [`LI_CCAP_STEAM_HAPTIC`] or [`LI_CCAP_STEAM_SETTINGS`]; there is
//! no SDP or serverinfo advertisement. A client with [`LI_CCAP_STEAM_ATTRIBUTES`] sends the
//! real controller's firmware attributes as an input packet ([`SteamAttributes`]).
//! Every value here is wire-visible and must match the client fork exactly.

/// Controller type of a Steam Controller in the arrival event.
pub const LI_CTYPE_STEAM: u8 = 0x04;

/// Arrival capability: capacitive grip touch via the grip touch flags.
pub const LI_CCAP_GRIP_SENSE: u16 = 0x200;
/// Arrival capability: capacitive stick touch via the stick touch flags.
pub const LI_CCAP_STICK_TOUCH: u16 = 0x400;
/// Arrival capability: the client replays [`SteamHaptic`] reports on the real pads.
pub const LI_CCAP_STEAM_HAPTIC: u16 = 0x800;
/// Arrival capability: independent physical pad and trigger clicks (the click flags).
pub const LI_CCAP_SC26_BUTTONS: u16 = 0x1000;
/// Arrival capability: the client applies [`SteamSettings`] writes to the real controller.
pub const LI_CCAP_STEAM_SETTINGS: u16 = 0x2000;
/// Arrival capability: the client sends [`SteamAttributes`] after the arrival event.
pub const LI_CCAP_STEAM_ATTRIBUTES: u16 = 0x4000;

/// Button flags above MISC_FLAG, in the upper half of the 32-bit button word (buttonFlags2
/// on the wire). Grip and stick touch are held, not pressed.
pub const LEFT_GRIP_TOUCH_FLAG: u32 = 0x0040_0000;
pub const RIGHT_GRIP_TOUCH_FLAG: u32 = 0x0080_0000;
pub const LEFT_STICK_TOUCH_FLAG: u32 = 0x0100_0000;
pub const RIGHT_STICK_TOUCH_FLAG: u32 = 0x0200_0000;
pub const LEFT_PAD_CLICK_FLAG: u32 = 0x0400_0000;
pub const RIGHT_PAD_CLICK_FLAG: u32 = 0x0800_0000;
pub const LEFT_TRIGGER_CLICK_FLAG: u32 = 0x1000_0000;
pub const RIGHT_TRIGGER_CLICK_FLAG: u32 = 0x2000_0000;

/// Control-stream message type of [`SteamHaptic`] (host to client).
pub const STEAM_HAPTIC_MESSAGE_TYPE: u16 = 0x5504;
/// Control-stream message type of [`SteamSettings`] (host to client).
pub const STEAM_SETTINGS_MESSAGE_TYPE: u16 = 0x5505;
/// Input packet magic of [`SteamAttributes`] (client to host).
pub const STEAM_ATTRIBUTES_MAGIC: u32 = 0x5500_0008;

/// Longest haptic report, id included.
pub const STEAM_HAPTIC_REPORT_MAX: usize = 12;
/// Encoded size of [`SteamHaptic`]: controller, length, report.
pub const STEAM_HAPTIC_BYTES: usize = 3 + STEAM_HAPTIC_REPORT_MAX;
/// Most (register, lo, hi) triples one settings write carries.
pub const STEAM_SETTINGS_TRIPLES_MAX: usize = 9;
/// Encoded size of [`SteamSettings`]: controller, command, count, triples.
pub const STEAM_SETTINGS_BYTES: usize = 4 + 3 * STEAM_SETTINGS_TRIPLES_MAX;
/// [`SteamSettings::command`] that writes the carried triples.
pub const STEAM_SETTINGS_SET_VALUES: u8 = 0x87;
/// [`SteamSettings::command`] that restores the firmware defaults (no triples).
pub const STEAM_SETTINGS_LOAD_DEFAULTS: u8 = 0x8E;
/// Encoded size of [`SteamAttributes`], the 8-byte input header included.
pub const STEAM_ATTRIBUTES_BYTES: usize = 84;

/// [`SteamAttributes::valid_mask`] bits: which fields were read from the controller.
pub const LI_SCATTR_FIRMWARE_BUILD_TIME: u8 = 0x01;
pub const LI_SCATTR_BOOTLOADER_BUILD_TIME: u8 = 0x02;
pub const LI_SCATTR_BOARD_REVISION: u8 = 0x04;
pub const LI_SCATTR_CAPABILITIES: u8 = 0x08;
pub const LI_SCATTR_RADIO_FW_BUILD_TIME: u8 = 0x10;
pub const LI_SCATTR_DONGLE_FW_BUILD_TIME: u8 = 0x20;
pub const LI_SCATTR_BUILD_ID: u8 = 0x40;
pub const LI_SCATTR_SERIALS: u8 = 0x80;
/// [`SteamAttributes::transport`] values (informational).
pub const LI_SCATTR_TRANSPORT_USB: u8 = 0;
pub const LI_SCATTR_TRANSPORT_BLE: u8 = 1;
pub const LI_SCATTR_TRANSPORT_PUCK: u8 = 2;

/// Why a Steam Controller message was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteamError {
    /// Fewer bytes than the message's fixed size.
    Short { len: usize },
    /// A haptic report longer than [`STEAM_HAPTIC_REPORT_MAX`].
    ReportTooLong { len: usize },
    /// A haptic report of no bytes: every report starts with its id.
    EmptyReport,
    /// More than [`STEAM_SETTINGS_TRIPLES_MAX`] settings triples.
    TooManyTriples { count: usize },
    /// A settings command other than [`STEAM_SETTINGS_SET_VALUES`] or
    /// [`STEAM_SETTINGS_LOAD_DEFAULTS`].
    UnknownCommand { command: u8 },
    /// [`STEAM_SETTINGS_LOAD_DEFAULTS`] carrying triples, which it never does.
    TriplesWithDefaults { count: usize },
    /// An attributes packet whose input header is not length 80 with the attributes magic.
    BadHeader { length: u32, magic: u32 },
}

/// A Steam Controller haptic output report (0x80 rumble, 0x81 pulse, 0x82 command, 0x83 LFO
/// tone, 0x84 sweep, 0x85 script), forwarded verbatim, id first. Never deduplicated: Steam
/// re-sends the same report to keep a buzz going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SteamHaptic {
    /// The controller number from the client's arrival event.
    pub controller: u16,
    length: u8,
    report: [u8; STEAM_HAPTIC_REPORT_MAX],
}

impl SteamHaptic {
    pub fn new(controller: u16, report: &[u8]) -> Result<Self, SteamError> {
        if report.len() > STEAM_HAPTIC_REPORT_MAX {
            return Err(SteamError::ReportTooLong { len: report.len() });
        }
        if report.is_empty() {
            return Err(SteamError::EmptyReport);
        }
        let mut padded = [0; STEAM_HAPTIC_REPORT_MAX];
        padded[..report.len()].copy_from_slice(report);
        Ok(Self {
            controller,
            length: report.len() as u8,
            report: padded,
        })
    }

    /// The report, id first, `length` bytes.
    pub fn report(&self) -> &[u8] {
        &self.report[..usize::from(self.length)]
    }

    /// Vibepollo's packed `control_steam_haptic_t` payload: controller (LE), length, the
    /// report zero-padded to 12 bytes.
    pub fn encode(&self) -> [u8; STEAM_HAPTIC_BYTES] {
        let mut out = [0; STEAM_HAPTIC_BYTES];
        out[0..2].copy_from_slice(&self.controller.to_le_bytes());
        out[2] = self.length;
        out[3..].copy_from_slice(&self.report);
        out
    }

    /// Bytes past the report's length are ignored, like trailing bytes.
    pub fn decode(data: &[u8]) -> Result<Self, SteamError> {
        if data.len() < STEAM_HAPTIC_BYTES {
            return Err(SteamError::Short { len: data.len() });
        }
        let length = usize::from(data[2]);
        if length > STEAM_HAPTIC_REPORT_MAX {
            return Err(SteamError::ReportTooLong { len: length });
        }
        Self::new(u16::from_le_bytes([data[0], data[1]]), &data[3..3 + length])
    }
}

/// A Steam Controller settings write: [`STEAM_SETTINGS_SET_VALUES`] with (register, lo, hi)
/// triples, or [`STEAM_SETTINGS_LOAD_DEFAULTS`] with none. Never coalesced: a chunk of a
/// longer write cannot replace its siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SteamSettings {
    /// The controller number from the client's arrival event.
    pub controller: u16,
    pub command: u8,
    count: u8,
    triples: [[u8; 3]; STEAM_SETTINGS_TRIPLES_MAX],
}

impl SteamSettings {
    pub fn new(controller: u16, command: u8, triples: &[[u8; 3]]) -> Result<Self, SteamError> {
        if triples.len() > STEAM_SETTINGS_TRIPLES_MAX {
            return Err(SteamError::TooManyTriples {
                count: triples.len(),
            });
        }
        match command {
            STEAM_SETTINGS_SET_VALUES => {}
            STEAM_SETTINGS_LOAD_DEFAULTS if triples.is_empty() => {}
            STEAM_SETTINGS_LOAD_DEFAULTS => {
                return Err(SteamError::TriplesWithDefaults {
                    count: triples.len(),
                })
            }
            _ => return Err(SteamError::UnknownCommand { command }),
        }
        let mut padded = [[0; 3]; STEAM_SETTINGS_TRIPLES_MAX];
        padded[..triples.len()].copy_from_slice(triples);
        Ok(Self {
            controller,
            command,
            count: triples.len() as u8,
            triples: padded,
        })
    }

    /// The (register, lo, hi) triples, `count` of them.
    pub fn triples(&self) -> &[[u8; 3]] {
        &self.triples[..usize::from(self.count)]
    }

    /// Vibepollo's packed `control_steam_settings_t` payload: controller (LE), command,
    /// count, the triples zero-padded to 27 bytes.
    pub fn encode(&self) -> [u8; STEAM_SETTINGS_BYTES] {
        let mut out = [0; STEAM_SETTINGS_BYTES];
        out[0..2].copy_from_slice(&self.controller.to_le_bytes());
        out[2] = self.command;
        out[3] = self.count;
        for (at, triple) in self.triples.iter().enumerate() {
            out[4 + 3 * at..7 + 3 * at].copy_from_slice(triple);
        }
        out
    }

    /// Triples past `count` are ignored, like trailing bytes.
    pub fn decode(data: &[u8]) -> Result<Self, SteamError> {
        if data.len() < STEAM_SETTINGS_BYTES {
            return Err(SteamError::Short { len: data.len() });
        }
        let count = usize::from(data[3]);
        if count > STEAM_SETTINGS_TRIPLES_MAX {
            return Err(SteamError::TooManyTriples { count });
        }
        let mut triples = [[0; 3]; STEAM_SETTINGS_TRIPLES_MAX];
        for (at, triple) in triples.iter_mut().enumerate().take(count) {
            triple.copy_from_slice(&data[4 + 3 * at..7 + 3 * at]);
        }
        Self::new(
            u16::from_le_bytes([data[0], data[1]]),
            data[2],
            &triples[..count],
        )
    }
}

/// The real Steam Controller's firmware attributes, read by the client at link setup so the
/// host's virtual controller reports the real firmware to Steam. Strings are NUL-padded and
/// unsanitised; the host checks them before use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteamAttributes {
    pub controller: u8,
    /// One of the `LI_SCATTR_TRANSPORT_*` values.
    pub transport: u8,
    /// `LI_SCATTR_*` bits for the fields actually read.
    pub valid_mask: u8,
    pub firmware_build_time: u32,
    pub bootloader_build_time: u32,
    pub board_revision: u32,
    pub capabilities: u32,
    pub radio_firmware_build_time: u32,
    pub dongle_firmware_build_time: u32,
    pub build_id: [u8; 16],
    pub unit_serial: [u8; 16],
    pub board_serial: [u8; 16],
}

impl SteamAttributes {
    /// The packed `SS_STEAM_CONTROLLER_ATTRIBUTES_PACKET`: input header (length 80 big-endian,
    /// magic little-endian), controller, transport, mask, a zero byte, six u32 LE, three
    /// 16-byte strings.
    pub fn encode(&self) -> [u8; STEAM_ATTRIBUTES_BYTES] {
        let mut out = [0; STEAM_ATTRIBUTES_BYTES];
        out[0..4].copy_from_slice(&(STEAM_ATTRIBUTES_BYTES as u32 - 4).to_be_bytes());
        out[4..8].copy_from_slice(&STEAM_ATTRIBUTES_MAGIC.to_le_bytes());
        out[8] = self.controller;
        out[9] = self.transport;
        out[10] = self.valid_mask;
        for (at, word) in [
            self.firmware_build_time,
            self.bootloader_build_time,
            self.board_revision,
            self.capabilities,
            self.radio_firmware_build_time,
            self.dongle_firmware_build_time,
        ]
        .into_iter()
        .enumerate()
        {
            out[12 + 4 * at..16 + 4 * at].copy_from_slice(&word.to_le_bytes());
        }
        out[36..52].copy_from_slice(&self.build_id);
        out[52..68].copy_from_slice(&self.unit_serial);
        out[68..84].copy_from_slice(&self.board_serial);
        out
    }

    /// Takes the whole packet, header included.
    pub fn decode(data: &[u8]) -> Result<Self, SteamError> {
        if data.len() < STEAM_ATTRIBUTES_BYTES {
            return Err(SteamError::Short { len: data.len() });
        }
        let length = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let magic = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if length != STEAM_ATTRIBUTES_BYTES as u32 - 4 || magic != STEAM_ATTRIBUTES_MAGIC {
            return Err(SteamError::BadHeader { length, magic });
        }
        let word =
            |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let string = |at: usize| {
            let mut out = [0; 16];
            out.copy_from_slice(&data[at..at + 16]);
            out
        };
        Ok(Self {
            controller: data[8],
            transport: data[9],
            valid_mask: data[10],
            firmware_build_time: word(12),
            bootloader_build_time: word(16),
            board_revision: word(20),
            capabilities: word(24),
            radio_firmware_build_time: word(28),
            dongle_firmware_build_time: word(32),
            build_id: string(36),
            unit_serial: string(52),
            board_serial: string(68),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn padded16(text: &[u8]) -> [u8; 16] {
        let mut out = [0; 16];
        out[..text.len()].copy_from_slice(text);
        out
    }

    #[test]
    fn haptic_matches_vibepollos_packed_layout() {
        // control_steam_haptic_t after its header: u16 id LE, u8 length, u8 report[12].
        let haptic = SteamHaptic::new(0x0102, &[0x81, 0x01, 0x20, 0x00, 0x40]).unwrap();
        let golden: [u8; 15] = [
            0x02, 0x01, 5, 0x81, 0x01, 0x20, 0x00, 0x40, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(haptic.encode(), golden);
        let decoded = SteamHaptic::decode(&golden).unwrap();
        assert_eq!(decoded, haptic);
        assert_eq!(decoded.report(), &[0x81, 0x01, 0x20, 0x00, 0x40]);

        let full = SteamHaptic::new(3, &[0x85; 12]).unwrap();
        assert_eq!(SteamHaptic::decode(&full.encode()), Ok(full));
        assert_eq!(full.report().len(), 12);
    }

    #[test]
    fn haptic_rejects_long_reports_and_short_payloads() {
        assert_eq!(
            SteamHaptic::new(0, &[0; 13]),
            Err(SteamError::ReportTooLong { len: 13 })
        );
        let mut bytes = SteamHaptic::new(0, &[0x80]).unwrap().encode();
        bytes[2] = 13;
        assert_eq!(
            SteamHaptic::decode(&bytes),
            Err(SteamError::ReportTooLong { len: 13 })
        );
        assert_eq!(
            SteamHaptic::decode(&bytes[..14]),
            Err(SteamError::Short { len: 14 })
        );
    }

    #[test]
    fn haptic_rejects_an_empty_report() {
        assert_eq!(SteamHaptic::new(0, &[]), Err(SteamError::EmptyReport));
        let mut bytes = SteamHaptic::new(0, &[0x80]).unwrap().encode();
        bytes[2] = 0;
        assert_eq!(SteamHaptic::decode(&bytes), Err(SteamError::EmptyReport));
    }

    #[test]
    fn haptic_ignores_bytes_past_its_length() {
        let mut bytes = SteamHaptic::new(1, &[0x80, 7]).unwrap().encode();
        bytes[10] = 0xff;
        let decoded = SteamHaptic::decode(&bytes).unwrap();
        assert_eq!(decoded.report(), &[0x80, 7]);
        assert_eq!(decoded, SteamHaptic::new(1, &[0x80, 7]).unwrap());
    }

    #[test]
    fn settings_match_vibepollos_packed_layout() {
        // control_steam_settings_t after its header: u16 id LE, u8 command, u8 count,
        // u8 triples[27].
        let settings = SteamSettings::new(
            2,
            STEAM_SETTINGS_SET_VALUES,
            &[[0x30, 0x10, 0x00], [0x31, 0xff, 0x7f]],
        )
        .unwrap();
        let mut golden = [0u8; 31];
        golden[..10].copy_from_slice(&[0x02, 0x00, 0x87, 2, 0x30, 0x10, 0x00, 0x31, 0xff, 0x7f]);
        assert_eq!(settings.encode(), golden);
        assert_eq!(SteamSettings::decode(&golden), Ok(settings));
        assert_eq!(
            settings.triples(),
            &[[0x30, 0x10, 0x00], [0x31, 0xff, 0x7f]]
        );

        let defaults = SteamSettings::new(0, STEAM_SETTINGS_LOAD_DEFAULTS, &[]).unwrap();
        let mut golden = [0u8; 31];
        golden[2] = 0x8e;
        assert_eq!(defaults.encode(), golden);
        assert_eq!(SteamSettings::decode(&golden), Ok(defaults));

        let full_triples: [[u8; 3]; 9] = core::array::from_fn(|at| [at as u8, 1, 2]);
        let full = SteamSettings::new(1, STEAM_SETTINGS_SET_VALUES, &full_triples).unwrap();
        assert_eq!(full.encode()[28..31], [8, 1, 2]);
        assert_eq!(SteamSettings::decode(&full.encode()), Ok(full));
    }

    #[test]
    fn settings_reject_too_many_triples_and_short_payloads() {
        assert_eq!(
            SteamSettings::new(0, STEAM_SETTINGS_SET_VALUES, &[[0; 3]; 10]),
            Err(SteamError::TooManyTriples { count: 10 })
        );
        let mut bytes = SteamSettings::new(0, STEAM_SETTINGS_SET_VALUES, &[[1, 2, 3]])
            .unwrap()
            .encode();
        bytes[3] = 10;
        assert_eq!(
            SteamSettings::decode(&bytes),
            Err(SteamError::TooManyTriples { count: 10 })
        );
        assert_eq!(
            SteamSettings::decode(&bytes[..30]),
            Err(SteamError::Short { len: 30 })
        );
    }

    #[test]
    fn settings_reject_unknown_commands() {
        for command in [0x00, 0x86, 0x88, 0x8d, 0x8f, 0xff] {
            assert_eq!(
                SteamSettings::new(0, command, &[]),
                Err(SteamError::UnknownCommand { command })
            );
        }
        let mut bytes = SteamSettings::new(0, STEAM_SETTINGS_SET_VALUES, &[[1, 2, 3]])
            .unwrap()
            .encode();
        bytes[2] = 0x86;
        assert_eq!(
            SteamSettings::decode(&bytes),
            Err(SteamError::UnknownCommand { command: 0x86 })
        );
    }

    #[test]
    fn settings_reject_load_defaults_with_triples() {
        assert_eq!(
            SteamSettings::new(0, STEAM_SETTINGS_LOAD_DEFAULTS, &[[1, 2, 3]]),
            Err(SteamError::TriplesWithDefaults { count: 1 })
        );
        let mut bytes = SteamSettings::new(0, STEAM_SETTINGS_SET_VALUES, &[[1, 2, 3]])
            .unwrap()
            .encode();
        bytes[2] = STEAM_SETTINGS_LOAD_DEFAULTS;
        assert_eq!(
            SteamSettings::decode(&bytes),
            Err(SteamError::TriplesWithDefaults { count: 1 })
        );
    }

    #[test]
    fn attributes_match_vibepollos_packed_layout() {
        let attributes = SteamAttributes {
            controller: 1,
            transport: LI_SCATTR_TRANSPORT_PUCK,
            valid_mask: 0xff,
            firmware_build_time: 0x6a4d_85e3,
            bootloader_build_time: 0x68d2_f92e,
            board_revision: 74,
            capabilities: 0x0102_0304,
            radio_firmware_build_time: 0x1112_1314,
            dongle_firmware_build_time: 0x2122_2324,
            build_id: padded16(b"26e1f7ef1955"),
            unit_serial: padded16(b"FXA0001"),
            board_serial: padded16(b"MXA0002"),
        };
        let bytes = attributes.encode();
        // NV_INPUT_HEADER: size excluding itself, big-endian, then the magic little-endian.
        assert_eq!(bytes[..8], [0, 0, 0, 80, 0x08, 0x00, 0x00, 0x55]);
        assert_eq!(bytes[8..12], [1, 2, 0xff, 0]);
        assert_eq!(bytes[12..16], [0xe3, 0x85, 0x4d, 0x6a]);
        assert_eq!(bytes[16..20], [0x2e, 0xf9, 0xd2, 0x68]);
        assert_eq!(bytes[20..24], [74, 0, 0, 0]);
        assert_eq!(bytes[24..28], [0x04, 0x03, 0x02, 0x01]);
        assert_eq!(bytes[28..32], [0x14, 0x13, 0x12, 0x11]);
        assert_eq!(bytes[32..36], [0x24, 0x23, 0x22, 0x21]);
        // buildId at header + 28, boardSerial at header + 60, per the header's static_asserts.
        assert_eq!(&bytes[36..52], &padded16(b"26e1f7ef1955"));
        assert_eq!(&bytes[52..68], &padded16(b"FXA0001"));
        assert_eq!(&bytes[68..84], &padded16(b"MXA0002"));
        assert_eq!(SteamAttributes::decode(&bytes), Ok(attributes));
    }

    #[test]
    fn attributes_reject_bad_headers_and_short_packets() {
        let bytes = SteamAttributes::default().encode();
        assert_eq!(
            SteamAttributes::decode(&bytes[..83]),
            Err(SteamError::Short { len: 83 })
        );
        let mut length = bytes;
        length[3] = 84;
        assert_eq!(
            SteamAttributes::decode(&length),
            Err(SteamError::BadHeader {
                length: 84,
                magic: STEAM_ATTRIBUTES_MAGIC
            })
        );
        let mut magic = bytes;
        magic[4] = 0x04;
        assert_eq!(
            SteamAttributes::decode(&magic),
            Err(SteamError::BadHeader {
                length: 80,
                magic: 0x5500_0004
            })
        );
    }

    #[test]
    fn capability_and_button_bits_stay_clear_of_each_other() {
        let caps = [
            LI_CCAP_GRIP_SENSE,
            LI_CCAP_STICK_TOUCH,
            LI_CCAP_STEAM_HAPTIC,
            LI_CCAP_SC26_BUTTONS,
            LI_CCAP_STEAM_SETTINGS,
            LI_CCAP_STEAM_ATTRIBUTES,
        ];
        assert_eq!(caps.iter().fold(0, |all, bit| all | bit), 0x7e00);
        let buttons = [
            LEFT_GRIP_TOUCH_FLAG,
            RIGHT_GRIP_TOUCH_FLAG,
            LEFT_STICK_TOUCH_FLAG,
            RIGHT_STICK_TOUCH_FLAG,
            LEFT_PAD_CLICK_FLAG,
            RIGHT_PAD_CLICK_FLAG,
            LEFT_TRIGGER_CLICK_FLAG,
            RIGHT_TRIGGER_CLICK_FLAG,
        ];
        assert_eq!(buttons.iter().fold(0, |all, bit| all | bit), 0x3fc0_0000);
    }
}
