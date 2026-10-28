//! HDR formats beyond stock HDR10, as bits of one mask.
//!
//! A host lists what it can encode in the serverinfo element [`SERVERINFO_FIELD`] and, per
//! stream, in the DESCRIBE attribute [`DESCRIBE_ATTRIBUTE`]. A client picks one bit with the
//! ANNOUNCE attribute [`ANNOUNCE_ATTRIBUTE`], and only together with stock HDR
//! (`x-nv-video[0].dynamicRangeMode:1`). Without it, or with 0, HDR is stock PQ.

/// HDR10: PQ transfer, BT.2020 primaries (what stock HDR already is).
pub const PQ: u8 = 1;
/// HLG: ARIB STD-B67 transfer in the VUI, BT.2020 primaries.
pub const HLG: u8 = 2;
/// Dolby Vision profile 8.1: an HDR10-compatible PQ base layer with RPU NAL units (type 62).
pub const DV_81: u8 = 4;
/// Dolby Vision profile 8.4: an HLG base layer with RPU NAL units.
pub const DV_84: u8 = 8;
/// Dolby Vision profile 10.1: an HDR10-compatible AV1 Main10 PQ base layer with each RPU in an
/// ITU-T T.35 metadata OBU (Dolby, provider-oriented code 0x800).
pub const DV_101: u8 = 16;
/// Every bit this version defines.
pub const ALL: u8 = PQ | HLG | DV_81 | DV_84 | DV_101;
/// The Dolby Vision bits.
pub const DOLBY_VISION: u8 = DV_81 | DV_84 | DV_101;
/// The Dolby Vision bits that need HEVC (profile 8).
pub const DOLBY_VISION_HEVC: u8 = DV_81 | DV_84;
/// The Dolby Vision bits that need AV1 (profile 10).
pub const DOLBY_VISION_AV1: u8 = DV_101;

/// Offer flag, not a format: HLG is also offered on PyroWave Main10 streams. Older hosts refuse
/// HLG on PyroWave, and the codec is known only in the ANNOUNCE, after DESCRIBE, so a client
/// learns it here first and still asks for [`HLG`]. Offer flags count down from the top bit and
/// formats up from the bottom, so the two never meet.
pub const PYROWAVE_HLG: u8 = 0x80;
/// Every offer flag this version defines; never a format, so `hdrFormat` refuses them.
pub const OFFER_FLAGS: u8 = PYROWAVE_HLG;

/// Serverinfo element: the formats the host can encode and its offer flags, decimal 0-255.
pub const SERVERINFO_FIELD: &str = "HdrFormats";
/// DESCRIBE attribute: the formats this stream can use.
pub const DESCRIBE_ATTRIBUTE: &str = "x-rl-video.hdrFormats";
/// ANNOUNCE attribute: the one format the client picked.
pub const ANNOUNCE_ATTRIBUTE: &str = "x-rl-video[0].hdrFormat";

/// `format` is exactly one bit this version defines.
pub fn single_bit(format: u8) -> bool {
    format & ALL == format && format.count_ones() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_distinct() {
        let bits = [PQ, HLG, DV_81, DV_84, DV_101];
        assert_eq!(bits.iter().fold(0, |all, bit| all | bit), ALL);
        assert_eq!(bits.iter().map(|bit| bit.count_ones()).sum::<u32>(), 5);
        assert_eq!(ALL, 31);
        assert_eq!(DOLBY_VISION, 28);
        assert_eq!(DOLBY_VISION_HEVC | DOLBY_VISION_AV1, DOLBY_VISION);
        assert_eq!(DOLBY_VISION_HEVC & DOLBY_VISION_AV1, 0);
    }

    #[test]
    fn single_bit_takes_one_known_format() {
        for bit in [PQ, HLG, DV_81, DV_84, DV_101] {
            assert!(single_bit(bit), "{bit}");
        }
        for mask in [0, 3, 5, 12, 15, 20, 31, 32, 0x80, 0xFF] {
            assert!(!single_bit(mask), "{mask}");
        }
    }

    #[test]
    fn offer_flags_are_not_formats() {
        assert_eq!(PYROWAVE_HLG, 0x80);
        assert_eq!(OFFER_FLAGS & ALL, 0);
        assert!(!single_bit(PYROWAVE_HLG));
        assert!(!single_bit(PYROWAVE_HLG | HLG));
    }
}
