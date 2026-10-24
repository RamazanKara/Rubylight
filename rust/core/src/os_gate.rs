//! Which Windows APIs the host may call on a given Windows build.
//!
//! Windows 10 22H2 and the early Windows 11 releases lack several APIs the
//! capture and virtual display paths use. Each one is gated here by the build
//! it first shipped in, with the reason the host logs when it steps around it,
//! and with a working path beside it (plan decision 1a): WGC without the
//! cursor, border or update-interval switches, Desktop Duplication, and the
//! physical display when no virtual display driver can load.
//!
//! An unknown build (`None`) is treated as capable, so a registry read that
//! fails never turns a working feature off; the call itself then reports its
//! own error as before.
//!
//! The minimum builds come from Microsoft's Windows SDK 10.0.26100 headers
//! (`UniversalApiContract` versions) and the IddCx versions page, which lists
//! the IddCx version each Windows release ships. They are untested on
//! hardware other than Windows 11 24H2/25H2 (26200).

/// Windows 10 1803: Windows Graphics Capture exists (`GraphicsCaptureItem`).
pub const WGC: u32 = 17134;
/// Windows 10 1903: `IGraphicsCaptureItemInterop::CreateForMonitor`, so WGC can
/// capture a whole display.
pub const WGC_MONITOR: u32 = 18362;
/// Windows 10 2004: `GraphicsCaptureSession.IsCursorCaptureEnabled`. Before it
/// the cursor is always in the picture.
pub const WGC_CURSOR_SWITCH: u32 = 19041;
/// `GraphicsCaptureSession.IsBorderRequired` (UniversalApiContract 12).
/// Before it the yellow capture border cannot be turned off.
pub const WGC_BORDER_SWITCH: u32 = 20348;
/// `MinUpdateInterval`, `DirtyRegionMode` and `IncludeSecondaryWindows`
/// (UniversalApiContract 19, Windows 11 24H2).
pub const WGC_UPDATE_INTERVAL: u32 = 26100;
/// Windows 10 1709: `IDXGIOutput5::DuplicateOutput1`, which names the formats
/// (FP16 for HDR); earlier builds only have the legacy 8-bit call.
pub const DUPLICATE_OUTPUT1: u32 = 16299;
/// Windows 10 1803: `IDXGIOutput6::GetDesc1`, the output's colour space and
/// luminance. Without it an HDR display cannot be told from an SDR one.
pub const OUTPUT_DESC1: u32 = 17134;
/// The newest build shipping an IddCx older than the driver's floor is 22621
/// (Windows 11 22H2, IddCx 1.9). Windows 11 23H2 (22631) ships IddCx 1.10, the
/// version the virtual display driver requires (`IddMinimumVersionRequired`).
pub const IDDCX_1_10: u32 = 22631;
/// Windows 11 21H2, the first Windows 11 build.
pub const WINDOWS_11: u32 = 22000;

fn at_least(build: Option<u32>, minimum: u32) -> bool {
    build.is_none_or(|build| build >= minimum)
}

/// A Windows feature and the build it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Wgc,
    WgcMonitor,
    WgcCursorSwitch,
    WgcBorderSwitch,
    WgcUpdateInterval,
    DuplicateOutput1,
    OutputDesc1,
    VirtualDisplay,
}

impl Feature {
    pub fn minimum(self) -> u32 {
        match self {
            Self::Wgc => WGC,
            Self::WgcMonitor => WGC_MONITOR,
            Self::WgcCursorSwitch => WGC_CURSOR_SWITCH,
            Self::WgcBorderSwitch => WGC_BORDER_SWITCH,
            Self::WgcUpdateInterval => WGC_UPDATE_INTERVAL,
            Self::DuplicateOutput1 => DUPLICATE_OUTPUT1,
            Self::OutputDesc1 => OUTPUT_DESC1,
            Self::VirtualDisplay => IDDCX_1_10,
        }
    }
    /// What the host does instead, for the log line.
    pub fn without(self) -> &'static str {
        match self {
            Self::Wgc => "Windows Graphics Capture is unavailable; use Desktop Duplication",
            Self::WgcMonitor => {
                "Windows Graphics Capture cannot capture a whole display; use Desktop Duplication"
            }
            Self::WgcCursorSwitch => {
                "the cursor is always part of the captured picture and cannot be switched"
            }
            Self::WgcBorderSwitch => {
                "Windows draws its capture border and the host cannot remove it"
            }
            Self::WgcUpdateInterval => {
                "capture frames arrive at Windows' own pace unless a servicing update added the property (the host still tries it)"
            }
            Self::DuplicateOutput1 => {
                "Desktop Duplication captures 8-bit SDR only, with no HDR format choice"
            }
            Self::OutputDesc1 => {
                "the host cannot read the display's HDR state and treats it as SDR"
            }
            Self::VirtualDisplay => {
                "the virtual display driver needs IddCx 1.10, so streams use the physical display"
            }
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Wgc => "Windows Graphics Capture",
            Self::WgcMonitor => "WGC display capture",
            Self::WgcCursorSwitch => "WGC cursor switch",
            Self::WgcBorderSwitch => "WGC border switch",
            Self::WgcUpdateInterval => "WGC minimum update interval",
            Self::DuplicateOutput1 => "DuplicateOutput1",
            Self::OutputDesc1 => "output HDR description",
            Self::VirtualDisplay => "virtual display driver",
        }
    }
}

/// Whether `feature` may be used on `build` (`None` when unknown).
pub fn supported(build: Option<u32>, feature: Feature) -> bool {
    at_least(build, feature.minimum())
}

/// The log line for a feature the build lacks, or None when it is available.
pub fn missing(build: Option<u32>, feature: Feature) -> Option<String> {
    (!supported(build, feature)).then(|| {
        format!(
            "{} needs Windows build {} or later; this PC is on build {}: {}.",
            feature.name(),
            feature.minimum(),
            build.map_or_else(|| "unknown".to_string(), |b| b.to_string()),
            feature.without()
        )
    })
}

/// Whether the virtual display driver can load: IddCx 1.10 ships in Windows 11
/// 23H2 and later. Windows 10 (IddCx 1.5), Windows 11 21H2 (1.8) and 22H2
/// (1.9) cannot bind a driver that requires 1.10.
pub fn virtual_display(build: Option<u32>) -> bool {
    supported(build, Feature::VirtualDisplay)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows releases as `(name, build)`.
    const WIN10_1709: (&str, u32) = ("Windows 10 1709", 16299);
    const WIN10_1809: (&str, u32) = ("Windows 10 1809", 17763);
    const WIN10_1903: (&str, u32) = ("Windows 10 1903", 18362);
    const WIN10_22H2: (&str, u32) = ("Windows 10 22H2", 19045);
    const WIN11_21H2: (&str, u32) = ("Windows 11 21H2", 22000);
    const WIN11_22H2: (&str, u32) = ("Windows 11 22H2", 22621);
    const WIN11_23H2: (&str, u32) = ("Windows 11 23H2", 22631);
    const WIN11_24H2: (&str, u32) = ("Windows 11 24H2", 26100);
    const WIN11_25H2: (&str, u32) = ("Windows 11 25H2", 26200);

    fn row(release: (&str, u32)) -> [bool; 8] {
        let build = Some(release.1);
        [
            supported(build, Feature::Wgc),
            supported(build, Feature::WgcMonitor),
            supported(build, Feature::WgcCursorSwitch),
            supported(build, Feature::WgcBorderSwitch),
            supported(build, Feature::WgcUpdateInterval),
            supported(build, Feature::DuplicateOutput1),
            supported(build, Feature::OutputDesc1),
            supported(build, Feature::VirtualDisplay),
        ]
    }

    #[test]
    fn each_release_gets_the_expected_apis() {
        // Wgc, Monitor, Cursor, Border, Interval, DuplicateOutput1, Desc1, VirtualDisplay
        assert_eq!(
            row(WIN10_1709),
            [false, false, false, false, false, true, false, false]
        );
        assert_eq!(
            row(WIN10_1809),
            [true, false, false, false, false, true, true, false]
        );
        assert_eq!(
            row(WIN10_1903),
            [true, true, false, false, false, true, true, false]
        );
        assert_eq!(
            row(WIN10_22H2),
            [true, true, true, false, false, true, true, false]
        );
        assert_eq!(
            row(WIN11_21H2),
            [true, true, true, true, false, true, true, false]
        );
        assert_eq!(
            row(WIN11_22H2),
            [true, true, true, true, false, true, true, false]
        );
        assert_eq!(
            row(WIN11_23H2),
            [true, true, true, true, false, true, true, true]
        );
        assert_eq!(
            row(WIN11_24H2),
            [true, true, true, true, true, true, true, true]
        );
        assert_eq!(
            row(WIN11_25H2),
            [true, true, true, true, true, true, true, true]
        );
    }

    #[test]
    fn an_unknown_build_is_treated_as_capable() {
        for feature in [
            Feature::Wgc,
            Feature::WgcMonitor,
            Feature::WgcCursorSwitch,
            Feature::WgcBorderSwitch,
            Feature::WgcUpdateInterval,
            Feature::DuplicateOutput1,
            Feature::OutputDesc1,
            Feature::VirtualDisplay,
        ] {
            assert!(supported(None, feature));
            assert_eq!(missing(None, feature), None);
        }
        assert!(virtual_display(None));
    }

    #[test]
    fn a_missing_feature_says_build_need_and_fallback() {
        let text = missing(Some(WIN10_22H2.1), Feature::WgcBorderSwitch).unwrap();
        assert!(text.contains("build 20348"), "{text}");
        assert!(text.contains("build 19045"), "{text}");
        assert!(text.contains("capture border"), "{text}");
        let text = missing(Some(WIN11_22H2.1), Feature::VirtualDisplay).unwrap();
        assert!(text.contains("IddCx 1.10"), "{text}");
        assert!(text.contains("physical display"), "{text}");
        assert_eq!(missing(Some(WIN11_25H2.1), Feature::VirtualDisplay), None);
    }

    #[test]
    fn the_virtual_display_floor_is_windows_11_23h2() {
        assert!(!virtual_display(Some(WIN10_22H2.1)));
        assert!(!virtual_display(Some(WIN11_21H2.1)));
        assert!(!virtual_display(Some(WIN11_22H2.1)));
        assert!(virtual_display(Some(WIN11_23H2.1)));
        assert!(virtual_display(Some(WIN11_25H2.1)));
        // The first Windows 11 build is below the floor on purpose.
        const { assert!(WINDOWS_11 < IDDCX_1_10) };
    }
}
