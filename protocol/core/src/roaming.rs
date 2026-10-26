//! Client roaming, wire format v1: keeping a session when the client's address changes.
//!
//! The client proves a new address with a path validation exchange on the host's control
//! UDP port, outside ENet: PROBE from the new address, CHALLENGE back to it, CONFIRM with the
//! challenge, then MIGRATED once the host has moved the session. Both sides intercept these
//! datagrams by their [`MAGIC`] before ENet sees them. Read as an ENet header the magic is
//! peer id 0xFFE without flags, which every ENet host drops, so a host without roaming
//! ignores them. A host offers roaming with the DESCRIBE attribute [`SDP_ATTRIBUTE`].
//!
//! Every datagram is [`DATAGRAM_BYTES`] long, little-endian, sealed with AES-128-GCM:
//!
//! | Off | Len | Field |
//! |---|---|---|
//! | 0 | 4 | magic `0F FE 52 4D` |
//! | 4 | 1 | version `1` |
//! | 5 | 1 | type: 1 PROBE c→h, 2 CHALLENGE h→c, 3 CONFIRM c→h, 4 MIGRATED h→c |
//! | 6 | 2 | reserved, 0 |
//! | 8 | 8 | session tag |
//! | 16 | 4 | seq, per sender, starts at 1 and strictly increases |
//! | 20 | 16 | GCM tag; the AAD is bytes 0..20 |
//! | 36 | 16 | ciphertext of the [`Payload`] |
//!
//! The GCM nonce is `type, direction, 0, 0, seq, 0, 0, 0, 0` ([`Header::nonce`]), so it
//! never repeats under one key while each sender's seq increases.
//!
//! Keys come from the per-launch `rikey` (the `/launch` AES key) with AES alone, so a host
//! needs nothing beyond the AES it already has:
//!
//! - roaming key = AES-128-ECB(rikey, [`KEY_BLOCK`])
//! - session tag = AES-128-ECB(rikey, [`TAG_BLOCK`])[0..8]
//!
//! This crate has no AES: it builds and checks everything around the cipher, and the host
//! and client seal and open with their own AES-GCM. [`vectors`] holds known answers both
//! must reproduce.

/// DESCRIBE attribute a host adds when it accepts roaming probes.
pub const SDP_ATTRIBUTE: &str = "a=x-rl-roaming:1";
/// First four bytes of every roaming datagram.
pub const MAGIC: [u8; 4] = [0x0F, 0xFE, 0x52, 0x4D];
/// Wire format version.
pub const VERSION: u8 = 1;
/// Size of every roaming datagram.
pub const DATAGRAM_BYTES: usize = 52;
/// Size of the authenticated clear header (the AAD): magic to seq.
pub const AAD_BYTES: usize = 20;
/// Size of the AES-GCM tag.
pub const GCM_TAG_BYTES: usize = 16;
/// Size of the sealed [`Payload`] (plaintext and ciphertext alike).
pub const PAYLOAD_BYTES: usize = 16;
/// Size of the AES-GCM nonce.
pub const NONCE_BYTES: usize = 12;
/// Size of the session tag.
pub const SESSION_TAG_BYTES: usize = 8;
/// AES-128-ECB input that derives the roaming key from the rikey.
pub const KEY_BLOCK: [u8; 16] = *b"RLROAM-KEY-v1\0\0\0";
/// AES-128-ECB input whose first [`SESSION_TAG_BYTES`] bytes under the rikey are the session
/// tag.
pub const TAG_BLOCK: [u8; 16] = *b"RLROAM-TAG-v1\0\0\0";
/// PROBE flag: the client has rebound its media sockets, so the host should expect pings
/// from new ports.
pub const PROBE_FLAG_MEDIA_REBOUND: u32 = 1;

/// Who sends a datagram. Part of the nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    ClientToHost = 1,
    HostToClient = 2,
}

/// Datagram type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Client to host, from the address to validate.
    Probe = 1,
    /// Host to client, to the probing address.
    Challenge = 2,
    /// Client to host, from the probing address, echoing the challenge.
    Confirm = 3,
    /// Host to client, once the session has moved.
    Migrated = 4,
}

impl Kind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Probe),
            2 => Some(Self::Challenge),
            3 => Some(Self::Confirm),
            4 => Some(Self::Migrated),
            _ => None,
        }
    }

    pub fn direction(self) -> Direction {
        match self {
            Self::Probe | Self::Confirm => Direction::ClientToHost,
            Self::Challenge | Self::Migrated => Direction::HostToClient,
        }
    }
}

/// Why a datagram that starts with [`MAGIC`] was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not exactly [`DATAGRAM_BYTES`] long.
    Length,
    /// Does not start with [`MAGIC`].
    Magic,
    /// A version other than [`VERSION`].
    Version,
    /// An unknown type.
    Kind,
    /// Reserved bytes are not zero.
    Reserved,
    /// Seq 0, which no sender uses.
    Seq,
}

/// True when `datagram` starts with [`MAGIC`]. A receiver intercepts every such datagram
/// before ENet, and drops it when [`Datagram::decode`] or the GCM open refuses it.
pub fn is_roaming(datagram: &[u8]) -> bool {
    datagram.starts_with(&MAGIC)
}

/// The clear part of a datagram, authenticated as the AAD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    /// [`TAG_BLOCK`] under the session's rikey; lets the host find the session.
    pub session_tag: [u8; SESSION_TAG_BYTES],
    /// Per sender, starting at 1 and strictly increasing.
    pub seq: u32,
}

impl Header {
    /// Bytes 0..20 of the datagram, which the GCM tag also covers.
    pub fn aad(&self) -> [u8; AAD_BYTES] {
        let mut out = [0; AAD_BYTES];
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        out[5] = self.kind as u8;
        out[8..16].copy_from_slice(&self.session_tag);
        out[16..20].copy_from_slice(&self.seq.to_le_bytes());
        out
    }

    /// AES-GCM nonce: type, direction, two zero bytes, seq, four zero bytes.
    pub fn nonce(&self) -> [u8; NONCE_BYTES] {
        let mut out = [0; NONCE_BYTES];
        out[0] = self.kind as u8;
        out[1] = self.kind.direction() as u8;
        out[4..8].copy_from_slice(&self.seq.to_le_bytes());
        out
    }
}

/// A whole datagram, still sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub header: Header,
    pub gcm_tag: [u8; GCM_TAG_BYTES],
    pub ciphertext: [u8; PAYLOAD_BYTES],
}

impl Datagram {
    /// Wire form: [`Header::aad`], GCM tag, ciphertext. Most AES-GCM APIs return the
    /// ciphertext followed by the tag; here the tag comes first.
    pub fn encode(&self) -> [u8; DATAGRAM_BYTES] {
        let mut out = [0; DATAGRAM_BYTES];
        out[..AAD_BYTES].copy_from_slice(&self.header.aad());
        out[AAD_BYTES..AAD_BYTES + GCM_TAG_BYTES].copy_from_slice(&self.gcm_tag);
        out[AAD_BYTES + GCM_TAG_BYTES..].copy_from_slice(&self.ciphertext);
        out
    }

    /// Parses the clear header strictly. The caller then opens the ciphertext with
    /// [`Header::nonce`] and the first [`AAD_BYTES`] bytes of `data` as AAD, and drops the
    /// datagram when that fails.
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() != DATAGRAM_BYTES {
            return Err(DecodeError::Length);
        }
        if !is_roaming(data) {
            return Err(DecodeError::Magic);
        }
        if data[4] != VERSION {
            return Err(DecodeError::Version);
        }
        let kind = Kind::from_u8(data[5]).ok_or(DecodeError::Kind)?;
        if data[6] != 0 || data[7] != 0 {
            return Err(DecodeError::Reserved);
        }
        let seq = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        if seq == 0 {
            return Err(DecodeError::Seq);
        }
        let mut datagram = Self {
            header: Header { kind, session_tag: [0; SESSION_TAG_BYTES], seq },
            gcm_tag: [0; GCM_TAG_BYTES],
            ciphertext: [0; PAYLOAD_BYTES],
        };
        datagram.header.session_tag.copy_from_slice(&data[8..16]);
        datagram.gcm_tag.copy_from_slice(&data[AAD_BYTES..AAD_BYTES + GCM_TAG_BYTES]);
        datagram.ciphertext.copy_from_slice(&data[AAD_BYTES + GCM_TAG_BYTES..]);
        Ok(datagram)
    }
}

/// The sealed content of each type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Payload {
    /// `nonce_c` is fresh for each probe burst and echoed in the CHALLENGE; `flags` holds
    /// [`PROBE_FLAG_MEDIA_REBOUND`]. Unknown flag bits are kept, and a host ignores them.
    Probe { nonce_c: u64, flags: u32 },
    /// The probe's `nonce_c` and a random challenge for the probing address.
    Challenge { nonce_c: u64, challenge: u64 },
    /// The challenge, proving the client received it at the probing address.
    Confirm { challenge: u64 },
    /// The confirmed challenge, once the session has moved.
    Migrated { challenge: u64 },
}

impl Payload {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Probe { .. } => Kind::Probe,
            Self::Challenge { .. } => Kind::Challenge,
            Self::Confirm { .. } => Kind::Confirm,
            Self::Migrated { .. } => Kind::Migrated,
        }
    }

    /// Plaintext: PROBE `nonce_c u64, flags u32, 0 u32`; CHALLENGE `nonce_c u64, challenge
    /// u64`; CONFIRM and MIGRATED `challenge u64, 0 u64`.
    pub fn encode(&self) -> [u8; PAYLOAD_BYTES] {
        let mut out = [0; PAYLOAD_BYTES];
        match *self {
            Self::Probe { nonce_c, flags } => {
                out[0..8].copy_from_slice(&nonce_c.to_le_bytes());
                out[8..12].copy_from_slice(&flags.to_le_bytes());
            }
            Self::Challenge { nonce_c, challenge } => {
                out[0..8].copy_from_slice(&nonce_c.to_le_bytes());
                out[8..16].copy_from_slice(&challenge.to_le_bytes());
            }
            Self::Confirm { challenge } | Self::Migrated { challenge } => {
                out[0..8].copy_from_slice(&challenge.to_le_bytes());
            }
        }
        out
    }

    /// Parses an opened plaintext of type `kind`; nonzero padding is refused.
    pub fn decode(kind: Kind, plaintext: &[u8; PAYLOAD_BYTES]) -> Option<Self> {
        let u64_at = |at: usize| {
            let mut word = [0; 8];
            word.copy_from_slice(&plaintext[at..at + 8]);
            u64::from_le_bytes(word)
        };
        let zero_from = |at: usize| plaintext[at..].iter().all(|&byte| byte == 0);
        match kind {
            Kind::Probe => zero_from(12).then(|| Self::Probe {
                nonce_c: u64_at(0),
                flags: u32::from_le_bytes([plaintext[8], plaintext[9], plaintext[10], plaintext[11]]),
            }),
            Kind::Challenge => Some(Self::Challenge { nonce_c: u64_at(0), challenge: u64_at(8) }),
            Kind::Confirm => zero_from(8).then(|| Self::Confirm { challenge: u64_at(0) }),
            Kind::Migrated => zero_from(8).then(|| Self::Migrated { challenge: u64_at(0) }),
        }
    }
}

/// Known answers every implementation must reproduce: a fixed rikey, the key and session tag
/// it derives, and one sealed datagram of each type. The client's seq runs PROBE 1, CONFIRM
/// 2; the host's CHALLENGE 1, MIGRATED 2. Computed with OpenSSL (Python `cryptography`) and
/// checked against RustCrypto's `aes` and `aes-gcm` in nightjar-core's
/// `tests/roaming_vectors.rs`.
///
/// | Input | Value |
/// |---|---|
/// | rikey | `000102030405060708090a0b0c0d0e0f` |
/// | roaming key | `6d553d3fa2e47b32ebb1609b11c82df2` |
/// | session tag | `52d89ae18eeb0232` |
/// | `nonce_c`, `challenge`, PROBE flags | `0x0123456789abcdef`, `0xfedcba9876543210`, 1 |
pub mod vectors {
    use super::DATAGRAM_BYTES;

    pub const RIKEY: [u8; 16] =
        [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f];
    pub const KEY: [u8; 16] =
        [0x6d, 0x55, 0x3d, 0x3f, 0xa2, 0xe4, 0x7b, 0x32, 0xeb, 0xb1, 0x60, 0x9b, 0x11, 0xc8, 0x2d, 0xf2];
    pub const SESSION_TAG: [u8; 8] = [0x52, 0xd8, 0x9a, 0xe1, 0x8e, 0xeb, 0x02, 0x32];
    pub const NONCE_C: u64 = 0x0123_4567_89ab_cdef;
    pub const CHALLENGE: u64 = 0xfedc_ba98_7654_3210;
    pub const PROBE_FLAGS: u32 = super::PROBE_FLAG_MEDIA_REBOUND;

    /// PROBE seq 1, `nonce_c` [`NONCE_C`], flags [`PROBE_FLAGS`].
    pub const PROBE_DATAGRAM: [u8; DATAGRAM_BYTES] = [
        0x0f, 0xfe, 0x52, 0x4d, 0x01, 0x01, 0x00, 0x00, 0x52, 0xd8, 0x9a, 0xe1, 0x8e, 0xeb, 0x02, 0x32, 0x01, 0x00,
        0x00, 0x00, 0x27, 0x5f, 0x7b, 0x4c, 0x17, 0xbd, 0x8f, 0xa9, 0xb0, 0x46, 0x75, 0x87, 0x69, 0x7c, 0x1a, 0x02,
        0x2c, 0x64, 0xe8, 0x2c, 0x54, 0xfd, 0xbd, 0x5b, 0x96, 0x45, 0xa4, 0x37, 0x3c, 0x75, 0x5a, 0xb8,
    ];
    /// CHALLENGE seq 1, `nonce_c` [`NONCE_C`], challenge [`CHALLENGE`].
    pub const CHALLENGE_DATAGRAM: [u8; DATAGRAM_BYTES] = [
        0x0f, 0xfe, 0x52, 0x4d, 0x01, 0x02, 0x00, 0x00, 0x52, 0xd8, 0x9a, 0xe1, 0x8e, 0xeb, 0x02, 0x32, 0x01, 0x00,
        0x00, 0x00, 0x29, 0x54, 0xb7, 0x79, 0x44, 0xaf, 0x32, 0xc4, 0xe1, 0x1d, 0x53, 0x83, 0x46, 0xb6, 0xa8, 0x1e,
        0xc5, 0x03, 0x59, 0x75, 0xa9, 0xad, 0x57, 0x39, 0xa4, 0xf0, 0x27, 0x40, 0xe8, 0x1f, 0x59, 0xe9,
    ];
    /// CONFIRM seq 2, challenge [`CHALLENGE`].
    pub const CONFIRM_DATAGRAM: [u8; DATAGRAM_BYTES] = [
        0x0f, 0xfe, 0x52, 0x4d, 0x01, 0x03, 0x00, 0x00, 0x52, 0xd8, 0x9a, 0xe1, 0x8e, 0xeb, 0x02, 0x32, 0x02, 0x00,
        0x00, 0x00, 0x49, 0xa2, 0x3e, 0x8d, 0x3d, 0x0a, 0xb0, 0xdb, 0x81, 0xe9, 0xb7, 0xc1, 0x06, 0x33, 0x4d, 0x23,
        0x8d, 0x38, 0xb8, 0x27, 0xe4, 0x6b, 0x1a, 0xbf, 0x7a, 0x14, 0x3a, 0x10, 0x63, 0xa2, 0x9d, 0x9c,
    ];
    /// MIGRATED seq 2, challenge [`CHALLENGE`].
    pub const MIGRATED_DATAGRAM: [u8; DATAGRAM_BYTES] = [
        0x0f, 0xfe, 0x52, 0x4d, 0x01, 0x04, 0x00, 0x00, 0x52, 0xd8, 0x9a, 0xe1, 0x8e, 0xeb, 0x02, 0x32, 0x02, 0x00,
        0x00, 0x00, 0x7e, 0x1a, 0xb6, 0x56, 0x0e, 0xb5, 0x7e, 0x87, 0xdd, 0x9d, 0x21, 0xd4, 0x6c, 0x85, 0xfb, 0x74,
        0x30, 0x20, 0x2c, 0x74, 0x41, 0xef, 0x5e, 0x9e, 0x4c, 0xbe, 0x14, 0x1a, 0xc2, 0xee, 0x8d, 0x80,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_header() -> Header {
        Header { kind: Kind::Probe, session_tag: vectors::SESSION_TAG, seq: 1 }
    }

    #[test]
    fn kdf_blocks_are_one_aes_block_each() {
        assert_eq!(&KEY_BLOCK, b"RLROAM-KEY-v1\x00\x00\x00");
        assert_eq!(&TAG_BLOCK, b"RLROAM-TAG-v1\x00\x00\x00");
    }

    #[test]
    fn magic_reads_as_an_enet_peer_no_host_has() {
        // ENet's header starts with a big-endian peer id whose top four bits are flags.
        let peer_id = u16::from_be_bytes([MAGIC[0], MAGIC[1]]);
        assert_eq!(peer_id & 0xF000, 0, "no ENet flag bits");
        assert_eq!(peer_id & 0x0FFF, 0xFFE);
    }

    #[test]
    fn aad_and_nonce_follow_the_table() {
        let header = Header { kind: Kind::Confirm, session_tag: [1, 2, 3, 4, 5, 6, 7, 8], seq: 0x0403_0201 };
        assert_eq!(header.aad(), [0x0f, 0xfe, 0x52, 0x4d, 1, 3, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0x01, 0x02, 0x03, 0x04]);
        assert_eq!(header.nonce(), [3, 1, 0, 0, 0x01, 0x02, 0x03, 0x04, 0, 0, 0, 0]);
        let migrated = Header { kind: Kind::Migrated, ..header };
        assert_eq!(migrated.nonce(), [4, 2, 0, 0, 0x01, 0x02, 0x03, 0x04, 0, 0, 0, 0]);
    }

    #[test]
    fn nonces_differ_by_type_and_direction_at_the_same_seq() {
        let kinds = [Kind::Probe, Kind::Challenge, Kind::Confirm, Kind::Migrated];
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                let at = |kind| Header { kind, session_tag: [0; 8], seq: 7 }.nonce();
                assert_ne!(at(*a), at(*b));
            }
        }
    }

    #[test]
    fn datagram_round_trips() {
        let datagram = Datagram { header: probe_header(), gcm_tag: [0xAA; 16], ciphertext: [0x55; 16] };
        let wire = datagram.encode();
        assert_eq!(wire.len(), DATAGRAM_BYTES);
        assert_eq!(&wire[..AAD_BYTES], &probe_header().aad());
        assert_eq!(&wire[20..36], &[0xAA; 16]);
        assert_eq!(&wire[36..], &[0x55; 16]);
        assert_eq!(Datagram::decode(&wire), Ok(datagram));
    }

    #[test]
    fn decode_is_strict() {
        let good = Datagram { header: probe_header(), gcm_tag: [0; 16], ciphertext: [0; 16] }.encode();
        assert_eq!(Datagram::decode(&good[..51]), Err(DecodeError::Length));
        let mut long = [0; DATAGRAM_BYTES + 1];
        long[..DATAGRAM_BYTES].copy_from_slice(&good);
        assert_eq!(Datagram::decode(&long), Err(DecodeError::Length));
        assert_eq!(Datagram::decode(&[]), Err(DecodeError::Length));

        let altered = |at: usize, value: u8| {
            let mut wire = good;
            wire[at] = value;
            Datagram::decode(&wire)
        };
        assert_eq!(altered(0, 0x0E), Err(DecodeError::Magic));
        assert_eq!(altered(3, 0x00), Err(DecodeError::Magic));
        assert_eq!(altered(4, 0), Err(DecodeError::Version));
        assert_eq!(altered(4, 2), Err(DecodeError::Version));
        assert_eq!(altered(5, 0), Err(DecodeError::Kind));
        assert_eq!(altered(5, 5), Err(DecodeError::Kind));
        assert_eq!(altered(6, 1), Err(DecodeError::Reserved));
        assert_eq!(altered(7, 0x80), Err(DecodeError::Reserved));
        assert_eq!(altered(16, 0), Err(DecodeError::Seq));
        // Seq is little-endian: a high byte alone is a valid seq.
        assert_eq!(altered(19, 1).map(|d| d.header.seq), Ok(0x0100_0001));
    }

    #[test]
    fn intercept_on_magic_alone() {
        assert!(is_roaming(&vectors::PROBE_DATAGRAM));
        assert!(is_roaming(&MAGIC), "a truncated roaming datagram is still intercepted, then refused");
        assert!(!is_roaming(&MAGIC[..3]));
        assert!(!is_roaming(&[0x00, 0x00, 0x52, 0x4d]));
    }

    #[test]
    fn kinds_map_to_directions() {
        for value in 0..=u8::MAX {
            match Kind::from_u8(value) {
                Some(kind) => {
                    assert_eq!(kind as u8, value);
                    let expected = if value % 2 == 1 { Direction::ClientToHost } else { Direction::HostToClient };
                    assert_eq!(kind.direction(), expected);
                }
                None => assert!(value == 0 || value > 4),
            }
        }
    }

    #[test]
    fn payloads_follow_the_table() {
        let probe = Payload::Probe { nonce_c: 0x0807_0605_0403_0201, flags: PROBE_FLAG_MEDIA_REBOUND };
        assert_eq!(probe.encode(), [1, 2, 3, 4, 5, 6, 7, 8, 1, 0, 0, 0, 0, 0, 0, 0]);
        let challenge = Payload::Challenge { nonce_c: 1, challenge: 0x0100_0000_0000_0002 };
        assert_eq!(challenge.encode(), [1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 1]);
        let confirm = Payload::Confirm { challenge: 9 };
        assert_eq!(confirm.encode(), [9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(Payload::Migrated { challenge: 9 }.encode(), confirm.encode());

        for payload in [probe, challenge, confirm, Payload::Migrated { challenge: u64::MAX }] {
            assert_eq!(Payload::decode(payload.kind(), &payload.encode()), Some(payload));
        }
    }

    #[test]
    fn payload_padding_must_be_zero() {
        let mut probe = Payload::Probe { nonce_c: 1, flags: 0 }.encode();
        probe[12] = 1;
        assert_eq!(Payload::decode(Kind::Probe, &probe), None);
        let mut confirm = Payload::Confirm { challenge: 1 }.encode();
        confirm[15] = 1;
        assert_eq!(Payload::decode(Kind::Confirm, &confirm), None);
        assert_eq!(Payload::decode(Kind::Migrated, &confirm), None);
        // Unknown probe flags are carried, not refused.
        let flagged = Payload::Probe { nonce_c: 1, flags: 0x8000_0001 };
        assert_eq!(Payload::decode(Kind::Probe, &flagged.encode()), Some(flagged));
    }

    #[test]
    fn vectors_decode_to_their_documented_headers() {
        let cases = [
            (&vectors::PROBE_DATAGRAM, Kind::Probe, 1),
            (&vectors::CHALLENGE_DATAGRAM, Kind::Challenge, 1),
            (&vectors::CONFIRM_DATAGRAM, Kind::Confirm, 2),
            (&vectors::MIGRATED_DATAGRAM, Kind::Migrated, 2),
        ];
        for (wire, kind, seq) in cases {
            let datagram = Datagram::decode(wire).unwrap();
            assert_eq!(datagram.header, Header { kind, session_tag: vectors::SESSION_TAG, seq });
            assert_eq!(&datagram.encode(), wire);
        }
    }
}
