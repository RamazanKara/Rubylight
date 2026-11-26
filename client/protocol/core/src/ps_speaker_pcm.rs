//! A PlayStation controller's speaker sound as Nonary's clients and Vibepollo carry it: speaker
//! PCM blocks in control message 0x5602 (host to client) and the 20-byte speaker binding,
//! input packet magic 0x55000009 (client to host).
//!
//! The wire format is Nonary's, read from `src/ControllerSpeakerPcm.h` of Nonary/moonlight-common-c
//! (merge 8101f711 of pull request 2, GPL-3.0-or-later) and `src/controller_speaker_pcm.h` of
//! Nonary/vibeshine (merge c90dadd2 of pull request 337, GPL-3.0-or-later); this is Rubylight's own
//! Rust implementation of it.
//!
//! Negotiation: the client sets [`SESSION_FEATURE`] in `x-ml-general.featureFlags` and, for each
//! PlayStation controller (arrival type [`CTYPE_PS`]) whose speaker it can play, sends a
//! [`Binding`] on the controller's reliable input channel right before the arrival, which carries
//! [`PS_SPEAKER_CAPABILITY`]. The host sends speaker blocks only on an encrypted control stream
//! (`x-nv-general.useReliableUdp` 13), unreliably on ENet channel [`CHANNEL`]. Every block echoes
//! the binding's owner, so the client drops blocks meant for a controller it no longer has.
//!
//! [`PS_SPEAKER_CAPABILITY`] is 0x4000, which a Steam Controller's arrival (type 4) uses for
//! [`crate::steam_controller::LI_CCAP_STEAM_ATTRIBUTES`]: read it only with the arrival's type,
//! through [`speaker_capable`].
//!
//! The magic is also Foundation Sunshine's one-finger touchpad packet (40 bytes, length field
//! 36). The two are told apart by their exact length alone: a binding is 20 bytes with length
//! field 16 and nothing else, a touchpad packet 40 with 36, so no packet is ever both, and a
//! client never sends a binding to a host that offers that touchpad packet.
//!
//! Speaker block (little-endian, [`HEADER_BYTES`] header): `version u8` (2), `format u8` (1,
//! 48 kHz S16LE stereo), `controller u16` (< 16), `generation u32` (not 0), `sequence u32`,
//! `sample_position u64` (stereo frames within the generation), `frames u16` (1 to
//! [`MAX_FRAMES`]), `flags u16` (only [`FLAG_DISCONTINUITY`]), `owner u64` (not 0), then
//! `frames * 4` sample bytes and nothing after them.
//!
//! Binding ([`BINDING_BYTES`]): the big-endian length after the length field (16), the magic
//! (little-endian), then `slot u16 LE` (< 16), `version u16 LE` (1) and `owner u64 LE` (not 0).

use alloc::vec::Vec;

/// Control message type of a speaker block, beside Nonary's actuator PCM (0x5601).
pub const PS_SPEAKER_PCM_MESSAGE_TYPE: u16 = 0x5602;
/// The ENet channel the host sends speaker blocks on, the one of Nonary's actuator PCM.
pub const CHANNEL: u8 = 8;
/// `x-ml-general.featureFlags` bit: the client plays speaker blocks.
pub const SESSION_FEATURE: u32 = 0x08;
/// Arrival capability `LI_CCAP_PS_SPEAKER_PCM`, meaningful only on a [`CTYPE_PS`] arrival.
pub const PS_SPEAKER_CAPABILITY: u16 = 0x4000;
/// Moonlight's `LI_CTYPE_PS`.
pub const CTYPE_PS: u8 = 0x02;
/// Moonlight's `LI_CTYPE_STEAM`, whose 0x4000 means Steam attributes.
pub const CTYPE_STEAM: u8 = 0x04;
/// Size of a speaker block's header.
pub const HEADER_BYTES: usize = 32;
/// Most stereo frames in one block (5 ms at 48 kHz); Vibepollo always sends this many.
pub const MAX_FRAMES: u16 = 240;
/// Bytes per stereo S16LE frame.
pub const FRAME_BYTES: usize = 4;
/// The largest speaker block.
pub const MAX_PAYLOAD: usize = HEADER_BYTES + MAX_FRAMES as usize * FRAME_BYTES;
/// The block's version byte. Version 1 had no owner and is refused.
pub const VERSION: u8 = 2;
/// 48 kHz S16LE stereo.
pub const FORMAT: u8 = 1;
/// The block follows a gap: what came before it is not contiguous with it.
pub const FLAG_DISCONTINUITY: u16 = 0x0001;
/// Input packet magic of a [`Binding`]; also Foundation Sunshine's touchpad packet's.
pub const BINDING_MAGIC: u32 = 0x5500_0009;
/// Size of a [`Binding`], its 4-byte length field included.
pub const BINDING_BYTES: usize = 20;
/// A binding's version.
pub const BINDING_VERSION: u16 = 1;
/// Controller numbers the protocol has.
const MAX_CONTROLLERS: u16 = 16;

/// Whether an arrival of `kind` with `capabilities` asks for speaker blocks: PlayStation only.
pub fn speaker_capable(kind: u8, capabilities: u16) -> bool {
    kind == CTYPE_PS && capabilities & PS_SPEAKER_CAPABILITY != 0
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from(le32(b, at)) | (u64::from(le32(b, at + 4)) << 32)
}

/// One speaker block, its samples borrowed from the packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeakerBlock<'a> {
    pub controller: u16,
    pub generation: u32,
    pub sequence: u32,
    pub sample_position: u64,
    pub flags: u16,
    pub owner: u64,
    /// Interleaved S16LE stereo, a whole number of frames.
    pub samples: &'a [u8],
}

impl<'a> SpeakerBlock<'a> {
    /// The block in `p`, the control message's payload, or `None` when it is not exactly one
    /// valid block (`MlSpeakerPcmValidate`).
    pub fn decode(p: &'a [u8]) -> Option<Self> {
        if p.len() < HEADER_BYTES || p[0] != VERSION || p[1] != FORMAT {
            return None;
        }
        let block = Self {
            controller: le16(p, 2),
            generation: le32(p, 4),
            sequence: le32(p, 8),
            sample_position: le64(p, 12),
            flags: le16(p, 22),
            owner: le64(p, 24),
            samples: &p[HEADER_BYTES..],
        };
        let frames = le16(p, 20);
        (block.valid() && usize::from(frames) * FRAME_BYTES == block.samples.len()).then_some(block)
    }

    /// Stereo frames in the block.
    pub fn frames(&self) -> u16 {
        (self.samples.len() / FRAME_BYTES) as u16
    }

    fn valid(&self) -> bool {
        let frames = self.samples.len() / FRAME_BYTES;
        self.controller < MAX_CONTROLLERS
            && self.generation != 0
            && self.owner != 0
            && self.flags & !FLAG_DISCONTINUITY == 0
            && self.samples.len().is_multiple_of(FRAME_BYTES)
            && (1..=usize::from(MAX_FRAMES)).contains(&frames)
            && self.sample_position <= u64::MAX - frames as u64
    }

    /// The control message payload, or `None` for a block [`Self::decode`] would refuse.
    pub fn encode(&self) -> Option<Vec<u8>> {
        if !self.valid() {
            return None;
        }
        let mut out = Vec::with_capacity(HEADER_BYTES + self.samples.len());
        out.extend_from_slice(&[VERSION, FORMAT]);
        out.extend_from_slice(&self.controller.to_le_bytes());
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&self.sample_position.to_le_bytes());
        out.extend_from_slice(&self.frames().to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.owner.to_le_bytes());
        out.extend_from_slice(self.samples);
        Some(out)
    }
}

/// Which client controller owns a slot's speaker: sent right before the slot's arrival.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub slot: u16,
    pub owner: u64,
}

impl Binding {
    /// The binding in `b`, a whole input packet with its length field, or `None` for anything
    /// that is not exactly a valid 20-byte binding: a touchpad packet with the same magic is
    /// 40 bytes and never one.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != BINDING_BYTES
            || b[..4] != [0, 0, 0, (BINDING_BYTES - 4) as u8]
            || le32(b, 4) != BINDING_MAGIC
            || le16(b, 10) != BINDING_VERSION
        {
            return None;
        }
        let binding = Self {
            slot: le16(b, 8),
            owner: le64(b, 12),
        };
        binding.valid().then_some(binding)
    }

    fn valid(&self) -> bool {
        self.slot < MAX_CONTROLLERS && self.owner != 0
    }

    /// The input packet, or `None` for a slot over 15 or a zero owner.
    pub fn encode(&self) -> Option<[u8; BINDING_BYTES]> {
        if !self.valid() {
            return None;
        }
        let mut out = [0; BINDING_BYTES];
        out[..4].copy_from_slice(&((BINDING_BYTES - 4) as u32).to_be_bytes());
        out[4..8].copy_from_slice(&BINDING_MAGIC.to_le_bytes());
        out[8..10].copy_from_slice(&self.slot.to_le_bytes());
        out[10..12].copy_from_slice(&BINDING_VERSION.to_le_bytes());
        out[12..].copy_from_slice(&self.owner.to_le_bytes());
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Nonary's `MlSpeakerPcmBuildBinding(p, 20, 3, 0x0102030405060708)`, byte for byte.
    const BINDING: [u8; 20] = [
        0, 0, 0, 16, 0x09, 0, 0, 0x55, 3, 0, 1, 0, 8, 7, 6, 5, 4, 3, 2, 1,
    ];

    fn block(frames: usize) -> Vec<u8> {
        let samples = vec![0x11; frames * FRAME_BYTES];
        SpeakerBlock {
            controller: 1,
            generation: 7,
            sequence: 9,
            sample_position: 480,
            flags: FLAG_DISCONTINUITY,
            owner: 0xDEAD_BEEF,
            samples: &samples,
        }
        .encode()
        .unwrap()
    }

    #[test]
    fn binding_matches_nonarys_layout() {
        let binding = Binding {
            slot: 3,
            owner: 0x0102_0304_0506_0708,
        };
        assert_eq!(binding.encode(), Some(BINDING));
        assert_eq!(Binding::decode(&BINDING), Some(binding));
    }

    #[test]
    fn binding_refuses_everything_but_one_exact_packet() {
        for (at, value) in [(3, 36), (3, 17), (4, 0x0A), (8, 16), (10, 2), (11, 1)] {
            let mut bad = BINDING;
            bad[at] = value;
            assert_eq!(Binding::decode(&bad), None, "byte {at} = {value}");
        }
        let mut zero_owner = BINDING;
        zero_owner[12..].fill(0);
        assert_eq!(Binding::decode(&zero_owner), None);
        for len in 0..64 {
            let mut sized = BINDING.to_vec();
            sized.resize(len, 0);
            assert_eq!(Binding::decode(&sized).is_some(), len == 20, "{len} bytes");
        }
        assert_eq!(Binding { slot: 16, owner: 1 }.encode(), None);
        assert_eq!(Binding { slot: 0, owner: 0 }.encode(), None);
    }

    #[test]
    fn block_matches_vibepollos_layout() {
        let wire = block(240);
        assert_eq!(wire.len(), MAX_PAYLOAD);
        assert_eq!(MAX_PAYLOAD, 992);
        assert_eq!(&wire[..4], &[2, 1, 1, 0]);
        assert_eq!(&wire[4..8], &7u32.to_le_bytes());
        assert_eq!(&wire[8..12], &9u32.to_le_bytes());
        assert_eq!(&wire[12..20], &480u64.to_le_bytes());
        assert_eq!(&wire[20..22], &240u16.to_le_bytes());
        assert_eq!(&wire[22..24], &1u16.to_le_bytes());
        assert_eq!(&wire[24..32], &0xDEAD_BEEFu64.to_le_bytes());
        let decoded = SpeakerBlock::decode(&wire).unwrap();
        assert_eq!(decoded.frames(), 240);
        assert_eq!(decoded.encode().unwrap(), wire);
        assert!(SpeakerBlock::decode(&block(1)).is_some());
    }

    #[test]
    fn block_refuses_what_nonarys_validator_refuses() {
        let good = block(4);
        let mut cases: Vec<Vec<u8>> = Vec::new();
        for (at, value) in [(0, 1), (0, 3), (1, 2), (2, 16), (22, 2), (23, 1)] {
            let mut bad = good.clone();
            bad[at] = value;
            cases.push(bad);
        }
        let mut zero_generation = good.clone();
        zero_generation[4..8].fill(0);
        cases.push(zero_generation);
        let mut zero_owner = good.clone();
        zero_owner[24..32].fill(0);
        cases.push(zero_owner);
        let mut zero_frames = good.clone();
        zero_frames[20..22].fill(0);
        zero_frames.truncate(HEADER_BYTES);
        cases.push(zero_frames);
        let mut overflow = good.clone();
        overflow[12..20].copy_from_slice(&(u64::MAX - 3).to_le_bytes());
        cases.push(overflow);
        let mut longer = good.clone();
        longer.push(0);
        cases.push(longer);
        let mut miscounted = good.clone();
        miscounted[20] = 5;
        cases.push(miscounted);
        cases.push(good[..HEADER_BYTES - 1].to_vec());
        let mut too_many = block(240);
        too_many[20..22].copy_from_slice(&241u16.to_le_bytes());
        too_many.extend_from_slice(&[0; 4]);
        cases.push(too_many);
        for case in cases {
            assert_eq!(
                SpeakerBlock::decode(&case),
                None,
                "{:?}",
                &case[..24.min(case.len())]
            );
        }
        let mut edge = good.clone();
        edge[12..20].copy_from_slice(&(u64::MAX - 4).to_le_bytes());
        assert!(SpeakerBlock::decode(&edge).is_some());
    }

    #[test]
    fn the_capability_is_playstation_only() {
        assert!(speaker_capable(CTYPE_PS, PS_SPEAKER_CAPABILITY));
        assert!(speaker_capable(CTYPE_PS, 0xFFFF));
        assert!(!speaker_capable(CTYPE_PS, !PS_SPEAKER_CAPABILITY));
        assert!(!speaker_capable(CTYPE_STEAM, PS_SPEAKER_CAPABILITY));
        for kind in [0, 1, 3, 4, 5, 0xFF] {
            assert!(!speaker_capable(kind, 0xFFFF), "type {kind}");
        }
        assert_eq!(
            PS_SPEAKER_CAPABILITY,
            crate::steam_controller::LI_CCAP_STEAM_ATTRIBUTES
        );
    }
}
