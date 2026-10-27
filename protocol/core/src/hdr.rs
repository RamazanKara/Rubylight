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
/// Every bit this version defines.
pub const ALL: u8 = PQ | HLG | DV_81 | DV_84;
/// The Dolby Vision bits; both need HEVC.
pub const DOLBY_VISION: u8 = DV_81 | DV_84;

/// Serverinfo element: the formats the host can encode, decimal 0-15.
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
        let bits = [PQ, HLG, DV_81, DV_84];
        assert_eq!(bits.iter().fold(0, |all, bit| all | bit), ALL);
        assert_eq!(bits.iter().map(|bit| bit.count_ones()).sum::<u32>(), 4);
        assert_eq!(ALL, 15);
        assert_eq!(DOLBY_VISION, 12);
    }

    #[test]
    fn single_bit_takes_one_known_format() {
        for bit in [PQ, HLG, DV_81, DV_84] {
            assert!(single_bit(bit), "{bit}");
        }
        for mask in [0, 3, 5, 12, 15, 16, 0x80, 0xFF] {
            assert!(!single_bit(mask), "{mask}");
        }
    }
}
