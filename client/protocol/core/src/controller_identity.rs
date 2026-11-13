//! Controller identity: which physical controller a client's gamepad is (input packet magic
//! 0x55000080), and the C button of a Switch 2 controller.
//!
//! Moonlight's arrival packet says a controller's family (Xbox, PlayStation, Nintendo) but
//! not which model it is. A client that knows the USB vendor and product IDs sends them in a
//! [`ControllerIdentity`] on the controller's gamepad channel just before its arrival packet,
//! so the host reads both in order and can give a Switch 2 Pro Controller a matching virtual
//! one. Such a client also sends the Switch 2 controllers' C button as [`BUTTON_C`] in its
//! controller packets. It does either only when the host's serverinfo has
//! [`SERVERINFO_FIELD`]; other hosts never see the packet or the bit.
//!
//! Layout (16 bytes): the big-endian length after the length field (12), the magic
//! (little-endian), then `controller u8`, `reserved u8` (0), `vendor u16 LE`, `product u16 LE`
//! and `reserved u16` (0).

/// Input packet magic of a [`ControllerIdentity`].
pub const CONTROLLER_IDENTITY_MAGIC: u32 = 0x5500_0080;
/// Size of a [`ControllerIdentity`], its 4-byte length included.
pub const CONTROLLER_IDENTITY_BYTES: usize = 16;
/// Serverinfo element: the host takes [`ControllerIdentity`] packets and [`BUTTON_C`].
pub const SERVERINFO_FIELD: &str = "ControllerIdentityVersion";
/// The version the host advertises in [`SERVERINFO_FIELD`].
pub const VERSION: u32 = 1;
/// Controller button flag of a Switch 2 controller's C button, after Moonlight's `MISC_FLAG`
/// (0x200000).
pub const BUTTON_C: u32 = 0x40_0000;
/// Nintendo's USB vendor ID.
pub const NINTENDO: u16 = 0x057E;
/// The Switch 2 Pro Controller's USB product ID.
pub const SWITCH2_PRO: u16 = 0x2069;
/// Controller numbers the protocol has.
const MAX_CONTROLLERS: u8 = 16;

/// Which controller a client's gamepad `controller` is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControllerIdentity {
    pub controller: u8,
    pub vendor: u16,
    pub product: u16,
}

/// Why [`ControllerIdentity::decode`] refused a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityError {
    /// Not [`CONTROLLER_IDENTITY_BYTES`] long, or its length field disagrees.
    Length,
    /// Another packet's magic.
    Magic { magic: u32 },
    /// A controller number past the 16 the protocol has.
    Controller { controller: u8 },
}

impl ControllerIdentity {
    /// Whether this is a Switch 2 Pro Controller.
    pub fn is_switch2_pro(&self) -> bool {
        is_switch2_pro(self.vendor, self.product)
    }

    pub fn encode(&self) -> [u8; CONTROLLER_IDENTITY_BYTES] {
        let mut out = [0; CONTROLLER_IDENTITY_BYTES];
        out[..4].copy_from_slice(&((CONTROLLER_IDENTITY_BYTES - 4) as u32).to_be_bytes());
        out[4..8].copy_from_slice(&CONTROLLER_IDENTITY_MAGIC.to_le_bytes());
        out[8] = self.controller;
        out[10..12].copy_from_slice(&self.vendor.to_le_bytes());
        out[12..14].copy_from_slice(&self.product.to_le_bytes());
        out
    }

    /// A whole packet, its length field included. Reserved bytes are ignored.
    pub fn decode(b: &[u8]) -> Result<Self, IdentityError> {
        if b.len() != CONTROLLER_IDENTITY_BYTES
            || u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize != b.len() - 4
        {
            return Err(IdentityError::Length);
        }
        let magic = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        if magic != CONTROLLER_IDENTITY_MAGIC {
            return Err(IdentityError::Magic { magic });
        }
        if b[8] >= MAX_CONTROLLERS {
            return Err(IdentityError::Controller { controller: b[8] });
        }
        Ok(Self {
            controller: b[8],
            vendor: u16::from_le_bytes([b[10], b[11]]),
            product: u16::from_le_bytes([b[12], b[13]]),
        })
    }
}

/// Whether USB `vendor` and `product` are a Switch 2 Pro Controller.
pub fn is_switch2_pro(vendor: u16, product: u16) -> bool {
    vendor == NINTENDO && product == SWITCH2_PRO
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Switch 2 Pro Controller as controller 2, byte for byte.
    const CAPTURED: [u8; 16] = [
        0x00, 0x00, 0x00, 0x0c, 0x80, 0x00, 0x00, 0x55, 0x02, 0x00, 0x7e, 0x05, 0x69, 0x20, 0x00,
        0x00,
    ];

    #[test]
    fn a_switch2_pro_identity_round_trips_the_reference_bytes() {
        let identity = ControllerIdentity {
            controller: 2,
            vendor: NINTENDO,
            product: SWITCH2_PRO,
        };
        assert_eq!(identity.encode(), CAPTURED);
        assert_eq!(ControllerIdentity::decode(&CAPTURED), Ok(identity));
        assert!(identity.is_switch2_pro());
        assert!(!is_switch2_pro(NINTENDO, 0x2009));
        assert!(!is_switch2_pro(0x054c, SWITCH2_PRO));
    }

    #[test]
    fn malformed_identities_are_refused() {
        assert_eq!(
            ControllerIdentity::decode(&CAPTURED[..15]),
            Err(IdentityError::Length)
        );
        let mut length = CAPTURED;
        length[3] = 13;
        assert_eq!(
            ControllerIdentity::decode(&length),
            Err(IdentityError::Length)
        );
        let mut magic = CAPTURED;
        magic[4] = 0x0b;
        assert_eq!(
            ControllerIdentity::decode(&magic),
            Err(IdentityError::Magic { magic: 0x5500_000B })
        );
        let mut controller = CAPTURED;
        controller[8] = 16;
        assert_eq!(
            ControllerIdentity::decode(&controller),
            Err(IdentityError::Controller { controller: 16 })
        );
        // Reserved bytes are ignored, so later versions can use them.
        let mut reserved = CAPTURED;
        reserved[9] = 1;
        reserved[15] = 1;
        assert!(ControllerIdentity::decode(&reserved).is_ok());
    }

    #[test]
    fn the_c_button_follows_moonlights_last_flag() {
        assert_eq!(BUTTON_C, 0x20_0000 << 1);
    }
}
