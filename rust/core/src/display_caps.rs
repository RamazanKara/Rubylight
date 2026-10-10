//! The client display's HDR luminance (control message 0x5531, [`DisplayCaps`]
//! from `rubylight-protocol`) as the HDR metadata the host sends and encodes.
//!
//! A client sends it at stream start and when its display changes. When it knows
//! its peak, an HDR stream describes the client's panel instead of the host's
//! display: peak as mastering display maximum and MaxCLL, frame-average as MaxFALL
//! and full-frame luminance, black level as mastering display minimum. A per-client
//! HDR profile, or a peak brightness chosen for the device or app on the host,
//! takes precedence: the user calibrated those for this client.
//!
//! The game renders for the host display's peak, not the client's. A client peak
//! below that would describe pixels brighter than the metadata says they can be:
//! the client then maps its peak 1:1 to its panel and clips everything above, so
//! such a report keeps the display's metadata. Android's "desired" luminance is
//! such a value on many phones (a Galaxy Z Fold 7 with a 2600-nit panel reports
//! 400-450 nits, varying with the panel's state).
use crate::{config::Config, hdr::Metadata, state::Client};
use rubylight_protocol::control::DisplayCaps;

pub use rubylight_protocol::control::DISPLAY_CAPS_MESSAGE_TYPE;

/// True when the host user chose this client's HDR luminance: a calibration
/// (`hdr_profile`) selected for the device, or a peak brightness override for
/// the device or app (which `config` carries as the runtime override marker of
/// `rtx_hdr_peak_brightness`).
pub fn host_override(config: &Config, client: &Client) -> bool {
    client
        .extra
        .get("hdr_profile")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|profile| !profile.trim().is_empty())
        || config.boolean(&crate::rtx_policy::marker("rtx_hdr_peak_brightness"), false)
}

/// `display` with the client's luminance, or `None` when the client does not know
/// its peak. Unknown average and black levels keep the display's values.
pub fn apply(display: Metadata, caps: &DisplayCaps) -> Option<Metadata> {
    let peak = caps.max_nits()?.clamp(1, u32::from(u16::MAX)) as u16;
    let mut metadata = display;
    metadata.maximum_nits = peak;
    metadata.max_cll = peak;
    if let Some(average) = caps.max_average_nits() {
        let average = average.clamp(1, u32::from(peak)) as u16;
        metadata.max_fall = average;
        metadata.full_frame_nits = average;
    }
    if caps.min_decimillinits != 0 {
        // Both in ten-thousandths of a nit.
        metadata.minimum = caps.min_decimillinits.min(u32::from(u16::MAX)) as u16;
    }
    Some(metadata)
}

/// What the host did with a client's display luminance, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The HDR metadata describes the client's display.
    Metadata,
    /// The stream is SDR; nothing to describe.
    SdrStream,
    /// The client did not know its peak.
    UnknownPeak,
    /// The client's peak is below the host display's, which the game renders for.
    BelowDisplay,
    /// The host user chose this client's HDR luminance.
    HostOverride,
    /// Kept until the stream has checked for a host override.
    Pending,
}

impl Applied {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "HDR metadata uses the client display",
            Self::SdrStream => "not applied: SDR stream",
            Self::UnknownPeak => "not applied: client peak unknown",
            Self::BelowDisplay => {
                "not applied: client peak below the host display's, which the game renders for"
            }
            Self::HostOverride => "not applied: HDR profile or peak brightness set on the host",
            Self::Pending => "kept until the stream starts",
        }
    }
}

/// A client's display luminance as the session took it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Update {
    pub caps: DisplayCaps,
    /// Differs from the client's previous report (or is its first).
    pub changed: bool,
    pub applied: Applied,
    /// The stream's HDR metadata now; `None` until the display's is known.
    pub metadata: Option<Metadata>,
}

/// The host display's metadata, the client's luminance and whether the host may
/// use it; [`HdrSource::effective`] is what the client is told and the encoder writes.
#[derive(Debug, Default)]
pub struct HdrSource {
    pub display: Option<Metadata>,
    pub caps: Option<DisplayCaps>,
    /// Whether the client's luminance may be used: `None` until the stream has
    /// checked for a host override, then false when one applies.
    pub allowed: Option<bool>,
}

impl HdrSource {
    /// How the client's luminance applies to a stream that is HDR or not.
    /// Before the display's metadata is known a usable report reads as
    /// [`Applied::Metadata`]; whether its peak is below the display's is decided
    /// once the display's metadata arrives.
    pub fn applied(&self, hdr: bool) -> Option<Applied> {
        let caps = self.caps.as_ref()?;
        Some(if !hdr {
            Applied::SdrStream
        } else if self.allowed.is_none() {
            Applied::Pending
        } else if self.allowed == Some(false) {
            Applied::HostOverride
        } else if let Some(peak) = caps.max_nits() {
            if self
                .display
                .is_some_and(|display| peak < u32::from(display.maximum_nits))
            {
                Applied::BelowDisplay
            } else {
                Applied::Metadata
            }
        } else {
            Applied::UnknownPeak
        })
    }

    /// Takes a 0x5531 payload. `None`, changing nothing, when it is malformed.
    pub fn on_payload(&mut self, payload: &[u8], hdr: bool) -> Option<Update> {
        let caps = DisplayCaps::decode(payload)?;
        let changed = self.caps != Some(caps);
        self.caps = Some(caps);
        Some(Update {
            caps,
            changed,
            applied: self.applied(hdr)?,
            metadata: self.effective(hdr),
        })
    }

    /// The metadata for the stream: the display's, with the client's luminance
    /// when it applies. `None` until the display's metadata is known.
    pub fn effective(&self, hdr: bool) -> Option<Metadata> {
        let display = self.display?;
        if self.applied(hdr) != Some(Applied::Metadata) {
            return Some(display);
        }
        self.caps
            .as_ref()
            .and_then(|caps| apply(display, caps))
            .or(Some(display))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold() -> DisplayCaps {
        // A Galaxy Z Fold 7 inner screen: 2600 nits peak, 1200 average, 0.0005 black.
        DisplayCaps {
            hdr: true,
            max_centinits: 260_000,
            max_average_centinits: 120_000,
            min_decimillinits: 5,
        }
    }

    fn client(profile: Option<&str>) -> Client {
        Client {
            name: "phone".into(),
            cert: String::new(),
            uuid: "phone".into(),
            perm: u32::MAX,
            enabled: true,
            extra: profile
                .map(|p| [("hdr_profile".to_string(), serde_json::json!(p))].into())
                .unwrap_or_default(),
        }
    }

    #[test]
    fn the_client_display_becomes_the_mastering_display_and_light_levels() {
        let display = Metadata::display(1000., 0.01, 600.);
        let metadata = apply(display, &fold()).unwrap();
        assert_eq!(metadata.maximum_nits, 2600);
        assert_eq!(metadata.max_cll, 2600);
        assert_eq!(metadata.max_fall, 1200);
        assert_eq!(metadata.full_frame_nits, 1200);
        assert_eq!(metadata.minimum, 5);
        assert_eq!(metadata.primaries, display.primaries);
        assert_eq!(metadata.white, display.white);
        // Unknown average and black keep the display's values.
        let peak_only = DisplayCaps {
            max_average_centinits: 0,
            min_decimillinits: 0,
            ..fold()
        };
        let metadata = apply(display, &peak_only).unwrap();
        assert_eq!(
            (metadata.maximum_nits, metadata.max_cll, metadata.max_fall),
            (2600, 2600, 0)
        );
        assert_eq!(metadata.minimum, display.minimum);
        assert_eq!(metadata.full_frame_nits, display.full_frame_nits);
        // Without a peak, nothing changes.
        let unknown = DisplayCaps {
            max_centinits: 0,
            ..fold()
        };
        assert_eq!(apply(display, &unknown), None);
        // Wire form: maximum, minimum, MaxCLL, MaxFALL, full frame at the end.
        let wire = apply(display, &fold()).unwrap().wire(true);
        let tail: Vec<_> = wire[17..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| u16::from_le_bytes(*v))
            .collect();
        assert_eq!(tail, [2600, 5, 2600, 1200, 1200]);
    }

    #[test]
    fn the_stream_uses_the_client_display_only_for_hdr_without_a_host_override() {
        let display = Metadata::display(1000., 0.01, 600.);
        let mut source = HdrSource::default();
        assert_eq!(source.effective(true), None);
        source.display = Some(display);
        // No report yet: the display's metadata.
        assert_eq!(source.effective(true), Some(display));
        assert_eq!(source.applied(true), None);
        source.caps = Some(fold());
        // Not yet allowed (the stream has not checked for an override), or overridden.
        assert_eq!(source.applied(true), Some(Applied::Pending));
        assert_eq!(source.effective(true), Some(display));
        source.allowed = Some(false);
        assert_eq!(source.applied(true), Some(Applied::HostOverride));
        assert_eq!(source.effective(true), Some(display));
        source.allowed = Some(true);
        assert_eq!(source.applied(true), Some(Applied::Metadata));
        assert_eq!(source.effective(true).unwrap().maximum_nits, 2600);
        assert_eq!(source.applied(false), Some(Applied::SdrStream));
        assert_eq!(source.effective(false), Some(display));
        source.caps = Some(DisplayCaps {
            max_centinits: 0,
            max_average_centinits: 0,
            ..fold()
        });
        assert_eq!(source.applied(true), Some(Applied::UnknownPeak));
        assert_eq!(source.effective(true), Some(display));
    }

    #[test]
    fn reports_are_decoded_compared_and_malformed_ones_ignored() {
        let display = Metadata::display(1000., 0.01, 600.);
        let mut source = HdrSource {
            allowed: Some(true),
            ..Default::default()
        };
        // Before the encoder has the display's metadata there is nothing to send yet.
        let first = source.on_payload(&fold().encode(), true).unwrap();
        assert!(first.changed);
        assert_eq!(first.applied, Applied::Metadata);
        assert_eq!(first.metadata, None);
        source.display = Some(display);
        let again = source.on_payload(&fold().encode(), true).unwrap();
        assert!(!again.changed);
        assert_eq!(again.metadata.unwrap().maximum_nits, 2600);
        let dimmer = DisplayCaps {
            max_centinits: 120_000,
            max_average_centinits: 40_000,
            ..fold()
        };
        let update = source.on_payload(&dimmer.encode(), true).unwrap();
        assert!(update.changed);
        assert_eq!(update.metadata.unwrap().maximum_nits, 1200);
        // Short, unknown-version and implausible payloads change nothing.
        let mut wrong_version = fold().encode();
        wrong_version[0] = 9;
        let implausible = DisplayCaps {
            max_centinits: 5_000_000,
            ..fold()
        };
        for payload in [
            &fold().encode()[..15],
            &wrong_version[..],
            &implausible.encode()[..],
            &[][..],
        ] {
            assert_eq!(source.on_payload(payload, true), None);
        }
        assert_eq!(source.caps, Some(dimmer));
        assert_eq!(source.effective(true).unwrap().maximum_nits, 1200);
    }

    #[test]
    fn a_client_peak_below_the_display_keeps_the_display_metadata() {
        // The game renders for the host display's 1000 nits. A Galaxy Z Fold 7 reports
        // Android's desired luminance, 400 nits, for its 2600-nit panel: describing
        // 400 nits would have the client clip everything the game draws above it.
        let display = Metadata::display(1000., 0.01, 600.);
        let android_desired = DisplayCaps {
            hdr: true,
            max_centinits: 40_000,
            max_average_centinits: 40_000,
            min_decimillinits: 5,
        };
        let mut source = HdrSource {
            allowed: Some(true),
            ..Default::default()
        };
        // Before the display's metadata is known there is nothing to compare with.
        let early = source.on_payload(&android_desired.encode(), true).unwrap();
        assert_eq!(early.applied, Applied::Metadata);
        assert_eq!(early.metadata, None);
        source.display = Some(display);
        assert_eq!(source.applied(true), Some(Applied::BelowDisplay));
        assert_eq!(source.effective(true), Some(display));
        // A change to 450 nits is still below: the display's metadata again.
        let update = source
            .on_payload(
                &DisplayCaps {
                    max_centinits: 45_000,
                    ..android_desired
                }
                .encode(),
                true,
            )
            .unwrap();
        assert!(update.changed);
        assert_eq!(update.applied, Applied::BelowDisplay);
        assert_eq!(update.metadata, Some(display));
        // Equal to the display's peak, or above it, the client's values apply.
        source.on_payload(
            &DisplayCaps {
                max_centinits: 100_000,
                ..android_desired
            }
            .encode(),
            true,
        );
        assert_eq!(source.applied(true), Some(Applied::Metadata));
        assert_eq!(source.effective(true).unwrap().max_fall, 400);
        source.on_payload(&fold().encode(), true);
        assert_eq!(source.effective(true).unwrap().maximum_nits, 2600);
        // An SDR stream, or a host override, still says so first.
        source.on_payload(&android_desired.encode(), true);
        assert_eq!(source.applied(false), Some(Applied::SdrStream));
        source.allowed = Some(false);
        assert_eq!(source.applied(true), Some(Applied::HostOverride));
    }

    #[test]
    fn a_host_profile_or_peak_override_takes_precedence() {
        let config = Config::default();
        assert!(!host_override(&config, &client(None)));
        assert!(!host_override(&config, &client(Some(""))));
        assert!(host_override(&config, &client(Some("TV.icm"))));
        // The global peak brightness alone is not a choice for this client.
        let mut config = Config::default();
        config
            .values
            .insert("rtx_hdr_peak_brightness".into(), "1500".into());
        assert!(!host_override(&config, &client(None)));
        config.values.insert(
            crate::rtx_policy::marker("rtx_hdr_peak_brightness"),
            "true".into(),
        );
        assert!(host_override(&config, &client(None)));
    }
}
