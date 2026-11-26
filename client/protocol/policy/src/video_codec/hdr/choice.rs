//! The user's "HDR format" choice, applied to what a client asks for: it masks the host's formats
//! as the client sees them when it picks one, never what the host offers, so DESCRIBE and ANNOUNCE
//! stay valid for every host.

use rubylight_protocol::hdr;
use serde::{Deserialize, Serialize};

/// `list_hdr_format` / `hdr_format`; the order is the ordinal Android passes through JNI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HdrFormatChoice {
    /// What the client picks by itself.
    #[default]
    Auto,
    /// Never Dolby Vision.
    NoDolbyVision,
    /// Plain HDR10: no Dolby Vision, no HLG, no HDR10+ or HDR Vivid.
    Hdr10,
}

impl HdrFormatChoice {
    /// Every choice, in the settings' order.
    pub const ALL: [Self; 3] = [Self::Auto, Self::NoDolbyVision, Self::Hdr10];

    /// The choice at `ordinal`, [`Self::Auto`] for anything else.
    pub fn from_ordinal(ordinal: i32) -> Self {
        match ordinal {
            1 => Self::NoDolbyVision,
            2 => Self::Hdr10,
            _ => Self::Auto,
        }
    }

    /// The host's `HdrFormats` as this choice may ask from them; -1 and 0 (none listed) stay.
    pub fn mask(self, host_formats: i32) -> i32 {
        let dolby = i32::from(hdr::DOLBY_VISION | hdr::DV_81_FULL_RANGE);
        let dropped = match self {
            Self::Auto => 0,
            Self::NoDolbyVision => dolby,
            Self::Hdr10 => dolby | i32::from(hdr::HLG),
        };
        if host_formats <= 0 {
            host_formats
        } else {
            host_formats & !dropped
        }
    }

    /// Whether the launch asks the host to leave out HDR10+ and HDR Vivid.
    pub fn static_metadata_only(self) -> bool {
        self == Self::Hdr10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_choice_masks_only_what_it_names() {
        let all = i32::from(hdr::ALL | hdr::OFFER_FLAGS);
        for host in [-1, 0, 1, 3, 31, all] {
            assert_eq!(HdrFormatChoice::Auto.mask(host), host);
        }
        assert_eq!(
            HdrFormatChoice::NoDolbyVision.mask(all),
            i32::from(hdr::PQ | hdr::HLG | hdr::PYROWAVE_HLG)
        );
        assert_eq!(
            HdrFormatChoice::Hdr10.mask(all),
            i32::from(hdr::PQ | hdr::PYROWAVE_HLG)
        );
        assert_eq!(HdrFormatChoice::Hdr10.mask(-1), -1);
        assert_eq!(
            HdrFormatChoice::from_ordinal(1),
            HdrFormatChoice::NoDolbyVision
        );
        assert_eq!(HdrFormatChoice::from_ordinal(9), HdrFormatChoice::Auto);
        assert!(HdrFormatChoice::Hdr10.static_metadata_only());
        assert!(!HdrFormatChoice::NoDolbyVision.static_metadata_only());
    }
}
