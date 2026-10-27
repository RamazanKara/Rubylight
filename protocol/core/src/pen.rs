//! Pen barrel roll: the pen packet with the pen's rotation about its own axis (input packet
//! magic 0x5500000B).
//!
//! The packet is the 36-byte 0x55000003 pen packet with its own magic, followed by the roll
//! (u16 LE, degrees 0-359 clockwise, or [`ROLL_UNKNOWN`]) and a reserved u16. The declared
//! big-endian length is 36. A client sends it instead of 0x55000003 only when the host has
//! both the serverinfo element [`SERVERINFO_FIELD`] and the DESCRIBE attribute
//! [`DESCRIBE_ATTRIBUTE`].

/// Input packet magic of the stock-layout pen packet this one extends.
pub const PEN_MAGIC: u32 = 0x5500_0003;
/// Size of that pen packet, its 4-byte length included.
pub const PEN_BYTES: usize = 36;
/// Input packet magic of a [`PenBarrelRoll`].
pub const PEN_BARREL_ROLL_MAGIC: u32 = 0x5500_000B;
/// Size of a [`PenBarrelRoll`], its 4-byte length included.
pub const PEN_BARREL_ROLL_BYTES: usize = 40;
/// A roll the pen does not report.
pub const ROLL_UNKNOWN: u16 = 0xFFFF;
/// Input magics other clients use for touchpad packets; never ours.
pub const RESERVED_MAGICS: [u32; 2] = [0x5500_0009, 0x5500_000A];
/// Serverinfo element: the host takes [`PenBarrelRoll`] packets.
pub const SERVERINFO_FIELD: &str = "PenBarrelRollVersion";
/// DESCRIBE attribute: this stream takes them. The name is shared with other clients.
pub const DESCRIBE_ATTRIBUTE: &str = "x-ss-general.penBarrelRoll";

/// A pen packet with its barrel roll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PenBarrelRoll {
    pen: [u8; PEN_BYTES],
    roll: u16,
}

impl PenBarrelRoll {
    /// `pen` is a whole 0x55000003 packet, length included; `None` if it is not one, or if
    /// `roll` is past 359 and not [`ROLL_UNKNOWN`].
    pub fn new(pen: &[u8], roll: u16) -> Option<Self> {
        let pen: [u8; PEN_BYTES] = pen.try_into().ok()?;
        let declared = u32::from_be_bytes([pen[0], pen[1], pen[2], pen[3]]) as usize;
        let magic = u32::from_le_bytes([pen[4], pen[5], pen[6], pen[7]]);
        (declared == PEN_BYTES - 4 && magic == PEN_MAGIC && roll_valid(roll))
            .then_some(Self { pen, roll })
    }

    /// The 0x55000003 packet, as a host without barrel roll would take it.
    pub fn pen(&self) -> &[u8; PEN_BYTES] {
        &self.pen
    }

    /// Degrees 0-359 clockwise, or [`ROLL_UNKNOWN`].
    pub fn roll(&self) -> u16 {
        self.roll
    }

    pub fn encode(&self) -> [u8; PEN_BARREL_ROLL_BYTES] {
        let mut out = [0; PEN_BARREL_ROLL_BYTES];
        out[..PEN_BYTES].copy_from_slice(&self.pen);
        out[..4].copy_from_slice(&((PEN_BARREL_ROLL_BYTES - 4) as u32).to_be_bytes());
        out[4..8].copy_from_slice(&PEN_BARREL_ROLL_MAGIC.to_le_bytes());
        out[36..38].copy_from_slice(&self.roll.to_le_bytes());
        out
    }

    /// Parses a packet whose declared length matches its size. Another magic, fewer than 40
    /// bytes and a roll past 359 that is not [`ROLL_UNKNOWN`] reject it; bytes past the 40
    /// are ignored.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < PEN_BARREL_ROLL_BYTES {
            return None;
        }
        let declared = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let magic = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        if declared != data.len() - 4 || magic != PEN_BARREL_ROLL_MAGIC {
            return None;
        }
        let mut pen = [0; PEN_BYTES];
        pen.copy_from_slice(&data[..PEN_BYTES]);
        pen[..4].copy_from_slice(&((PEN_BYTES - 4) as u32).to_be_bytes());
        pen[4..8].copy_from_slice(&PEN_MAGIC.to_le_bytes());
        let roll = u16::from_le_bytes([data[36], data[37]]);
        roll_valid(roll).then_some(Self { pen, roll })
    }
}

fn roll_valid(roll: u16) -> bool {
    roll < 360 || roll == ROLL_UNKNOWN
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen_packet() -> [u8; PEN_BYTES] {
        let mut pen = [0; PEN_BYTES];
        pen[..4].copy_from_slice(&32u32.to_be_bytes());
        pen[4..8].copy_from_slice(&PEN_MAGIC.to_le_bytes());
        pen[8] = 1; // event
        pen[9] = 1; // tool
        pen[12..16].copy_from_slice(&0.25f32.to_le_bytes());
        pen[16..20].copy_from_slice(&0.75f32.to_le_bytes());
        pen[24..26].copy_from_slice(&90u16.to_le_bytes());
        pen[26] = 30;
        pen
    }

    #[test]
    fn packets_round_trip_around_the_pen_packet() {
        let packet = PenBarrelRoll::new(&pen_packet(), 270).unwrap();
        let wire = packet.encode();
        assert_eq!(wire[..4], 36u32.to_be_bytes());
        assert_eq!(wire[4..8], [0x0B, 0, 0, 0x55]);
        assert_eq!(wire[8..36], pen_packet()[8..]);
        assert_eq!(wire[36..], [0x0E, 0x01, 0, 0]);
        let decoded = PenBarrelRoll::decode(&wire).unwrap();
        assert_eq!(decoded, packet);
        assert_eq!(decoded.pen(), &pen_packet());
        assert_eq!(decoded.roll(), 270);
        for roll in [0, 359, ROLL_UNKNOWN] {
            let packet = PenBarrelRoll::new(&pen_packet(), roll).unwrap();
            assert_eq!(PenBarrelRoll::decode(&packet.encode()), Some(packet));
        }
        // A longer packet that says so.
        let mut longer = wire.to_vec();
        longer.extend_from_slice(&[0; 4]);
        longer[..4].copy_from_slice(&40u32.to_be_bytes());
        assert_eq!(PenBarrelRoll::decode(&longer), Some(packet));
    }

    #[test]
    fn bad_packets_are_refused() {
        assert_eq!(PenBarrelRoll::new(&pen_packet(), 360), None);
        assert_eq!(PenBarrelRoll::new(&pen_packet()[..35], 0), None);
        let mut other = pen_packet();
        other[4] = 0x02;
        assert_eq!(PenBarrelRoll::new(&other, 0), None, "a touch packet");
        let mut misdeclared = pen_packet();
        misdeclared[3] = 36;
        assert_eq!(PenBarrelRoll::new(&misdeclared, 0), None);

        let wire = PenBarrelRoll::new(&pen_packet(), 10).unwrap().encode();
        assert_eq!(PenBarrelRoll::decode(&wire[..39]), None);
        let mut stock = wire;
        stock[4] = 0x03;
        assert_eq!(PenBarrelRoll::decode(&stock), None, "stock pen magic");
        let mut short_declared = wire;
        short_declared[3] = 32;
        assert_eq!(PenBarrelRoll::decode(&short_declared), None);
        let mut bad_roll = wire;
        bad_roll[36..38].copy_from_slice(&360u16.to_le_bytes());
        assert_eq!(PenBarrelRoll::decode(&bad_roll), None);
    }

    #[test]
    fn magics_stay_clear_of_the_reserved_ones() {
        assert!(!RESERVED_MAGICS.contains(&PEN_BARREL_ROLL_MAGIC));
        assert!(!RESERVED_MAGICS.contains(&PEN_MAGIC));
        assert_eq!(PEN_BARREL_ROLL_BYTES, PEN_BYTES + 4);
    }
}
