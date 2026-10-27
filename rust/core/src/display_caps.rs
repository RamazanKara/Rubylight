//! Client display luminance (0x5531) is diagnostic information, not HDR content metadata.
//!
//! HDR10 mastering luminance and MaxCLL/MaxFALL describe the source and its pixels.
//! Replacing them with a receiver's changing brightness does not tone-map those
//! pixels. It instead makes the receiver change tone mapping (and some Android
//! clients restart their decoder) while the source has not changed.
use crate::hdr::Metadata;
use rubylight_protocol::control::DisplayCaps;

pub use rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE;

#[derive(Debug, PartialEq, Eq)]
pub struct Update {
    pub caps: DisplayCaps,
    pub changed: bool,
}

/// Source metadata and the last valid receiver capability report. Only source
/// changes may update the HDR metadata sent in the control stream and bitstream.
#[derive(Debug, Default)]
pub struct HdrSource {
    pub display: Option<Metadata>,
    pub caps: Option<DisplayCaps>,
}

impl HdrSource {
    /// Malformed reports leave the previous capability report untouched.
    pub fn on_payload(&mut self, payload: &[u8]) -> Option<Update> {
        let caps = DisplayCaps::decode(payload)?;
        let changed = self.caps != Some(caps);
        self.caps = Some(caps);
        Some(Update { caps, changed })
    }

    /// The display peak dynamic metadata (`hdr_dynamic`) is written for: the receiver's
    /// reported peak, else the source display's, else 1000 nits. Dynamic metadata names its
    /// target display per frame, so following the receiver there changes no static value.
    pub fn target_nits(&self) -> u16 {
        crate::hdr_dynamic::target_display_nits(
            self.caps
                .and_then(|caps| caps.max_nits())
                .map(|nits| nits as f32),
            self.display.map(|display| f32::from(display.maximum_nits)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> DisplayCaps {
        DisplayCaps {
            hdr: true,
            max_centinits: 260_000,
            max_average_centinits: 120_000,
            min_decimillinits: 5,
        }
    }

    #[test]
    fn reports_are_compared_without_changing_source_metadata() {
        let display = Metadata::display(1000., 0.01, 600.);
        let mut source = HdrSource {
            display: Some(display),
            ..Default::default()
        };
        assert!(source.on_payload(&panel().encode()).unwrap().changed);
        assert!(!source.on_payload(&panel().encode()).unwrap().changed);
        for peak in [0, 40_000, 100_000, 260_000] {
            let caps = DisplayCaps {
                max_centinits: peak,
                max_average_centinits: 0,
                ..panel()
            };
            source.on_payload(&caps.encode()).unwrap();
            assert_eq!(source.display, Some(display));
        }
    }

    #[test]
    fn early_and_malformed_reports_cannot_supply_source_metadata() {
        let mut source = HdrSource::default();
        source.on_payload(&panel().encode()).unwrap();
        assert_eq!(source.display, None);
        let mut wrong_version = panel().encode();
        wrong_version[0] = 9;
        let implausible = DisplayCaps {
            max_centinits: 5_000_000,
            ..panel()
        };
        for payload in [
            &panel().encode()[..8],
            &wrong_version[..],
            &implausible.encode()[..],
            &[][..],
        ] {
            assert_eq!(source.on_payload(payload), None);
            assert_eq!(source.caps, Some(panel()));
            assert_eq!(source.display, None);
        }
    }

    #[test]
    fn dynamic_metadata_targets_the_receiver_then_the_source_display() {
        let mut source = HdrSource::default();
        assert_eq!(source.target_nits(), 1000);
        source.display = Some(Metadata::display(600., 0.01, 300.));
        assert_eq!(source.target_nits(), 600);
        source.on_payload(&panel().encode()).unwrap();
        assert_eq!(source.target_nits(), 2600);
        // A receiver that reports no peak leaves the source display's.
        let unknown = DisplayCaps {
            max_centinits: 0,
            ..panel()
        };
        source.on_payload(&unknown.encode()).unwrap();
        assert_eq!(source.target_nits(), 600);
        // Reading the target changes nothing the static metadata comes from.
        assert_eq!(source.display, Some(Metadata::display(600., 0.01, 300.)));
    }
}
