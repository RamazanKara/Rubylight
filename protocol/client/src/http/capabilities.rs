//! What one host can do, judged only from its serverinfo reply. Port of Nightjar's
//! HostCapabilities.java: the family names the host for labels and protocol quirks; features
//! alone decide what a client offers.

use super::xml::Element;

/// The host software, for labels and wire-format quirks. Never gates a feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    GameStream,
    Sunshine,
    Apollo,
    Vibepollo,
    Rubylight,
    Unknown,
}

/// Something a host can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Permissions,
    OtpPairing,
    Clipboard,
    ServerCommands,
    AppUuids,
    LaunchByUuid,
    VirtualDisplay,
    VirtualDisplayHdr,
    HostDisplayModes,
    FractionalRefresh,
    HostVrr,
    RuntimeBitrate,
    PyroWave,
    PyroWaveRecords,
    PyroWaveBandwidthProbe,
    HardwareMetrics,
    NetworkPath,
    FrameLimiterStatus,
    ControllerPreparation,
    RemoteSessionTiles,
    HostConfirmsReplacement,
    HostTiming,
    WebUi,
    CursorShape,
    /// The Rubylight native API (`/api/native/v1`): library, session actions, Radeon state.
    NativeApi,
}

impl Feature {
    pub const ALL: [Feature; 25] = [
        Feature::Permissions,
        Feature::OtpPairing,
        Feature::Clipboard,
        Feature::ServerCommands,
        Feature::AppUuids,
        Feature::LaunchByUuid,
        Feature::VirtualDisplay,
        Feature::VirtualDisplayHdr,
        Feature::HostDisplayModes,
        Feature::FractionalRefresh,
        Feature::HostVrr,
        Feature::RuntimeBitrate,
        Feature::PyroWave,
        Feature::PyroWaveRecords,
        Feature::PyroWaveBandwidthProbe,
        Feature::HardwareMetrics,
        Feature::NetworkPath,
        Feature::FrameLimiterStatus,
        Feature::ControllerPreparation,
        Feature::RemoteSessionTiles,
        Feature::HostConfirmsReplacement,
        Feature::HostTiming,
        Feature::WebUi,
        Feature::CursorShape,
        Feature::NativeApi,
    ];
}

/// What would make an unsupported feature available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The host lacks something a newer version has. For a Rubylight host without the native
    /// API this is a 2.6.x host, and NVIDIA and Intel users cannot update to 3.0 (it is
    /// AMD-only), so the UI must not say "update the host" for it. Word it as "this host
    /// doesn't offer it".
    UpdateHost,
    Pair,
    ViewPermission,
    ClipboardPermission,
    CommandPermission,
    NoCommands,
    VirtualDisplayOff,
    VirtualDisplayDriver,
    HardwareMetricsOff,
}

/// Whether a host offers a feature; an unsupported one may carry what would change that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Availability {
    Supported,
    /// `None`: nothing the user could change on the host alters it.
    Unsupported(Option<Reason>),
    Unknown,
}

impl Availability {
    const NEVER: Availability = Availability::Unsupported(None);

    pub fn is_supported(self) -> bool {
        self == Availability::Supported
    }

    pub fn is_never(self) -> bool {
        self == Availability::NEVER
    }
}

/// ServerCodecModeSupport bits of the four PyroWave profiles.
pub const PYROWAVE_CODEC_BITS: u32 = 0x0080_0000 | 0x0100_0000 | 0x0200_0000 | 0x0400_0000;
/// Default bandwidth probe size; a host advertising another size gets no probe.
pub const PYROWAVE_BANDWIDTH_PROBE_BYTES: u32 = 32 * 1024 * 1024;

pub const PERMISSION_CONTROLLER: u32 = 1 << 8;
pub const PERMISSION_INPUT: u32 = 0x0000_1f00;
pub const PERMISSION_CLIPBOARD_SET: u32 = 1 << 16;
pub const PERMISSION_CLIPBOARD_READ: u32 = 1 << 17;
pub const PERMISSION_SERVER_COMMAND: u32 = 1 << 20;
pub const PERMISSION_LIST: u32 = 1 << 24;
pub const PERMISSION_VIEW: u32 = 1 << 25;
pub const PERMISSION_LAUNCH: u32 = 1 << 26;

/// The serverinfo fields capabilities are derived from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Evidence {
    pub game_stream: bool,
    pub sunshine_version: bool,
    pub paired: bool,
    pub rust_host_version: Option<String>,
    /// -1 when the host reports no permissions.
    pub permission: i64,
    pub server_commands: u32,
    pub virtual_display_capable: Option<bool>,
    pub virtual_display_driver_ready: Option<bool>,
    pub virtual_display_hdr_capable: Option<bool>,
    pub frame_limiter: Option<bool>,
    pub pyrowave_fields: bool,
    pub pyrowave_probe_bytes: u32,
    pub pyrowave_codecs: u32,
    pub encoder_bitrate: bool,
    pub hardware_metrics_version: i32,
    pub network_path_version: i32,
    pub controller_preparation: bool,
    pub current_game_uuid: bool,
    pub cursor_shape_version: i32,
    /// -1 when the host has no native API, 0 when it has one but not for this request (plain
    /// HTTP or an unpaired certificate), 1 or more for the highest version it serves.
    pub native_api_version: i32,
}

/// A host's capabilities. [`HostCapabilities::UNKNOWN`] is a host never reached.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostCapabilities {
    evidence: Option<Evidence>,
}

fn unsigned(value: &str, max: u64) -> u64 {
    match value.trim().parse::<i64>() {
        Ok(v) if v >= 0 && (v as u64) <= max => v as u64,
        _ => 0,
    }
}

fn flag(value: &str) -> Option<bool> {
    let value = value.trim();
    if value == "1" || value.eq_ignore_ascii_case("true") {
        Some(true)
    } else if value == "0" || value.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// "2.2.21", "2.0.0-rc.19" or "3.0.0-beta.1" as [major, minor, patch, pre-release], a release
/// after its pre-releases, which rank alpha, beta, rc (the stage in the millions of the last part).
pub fn parse_rubylight_version(version: &str) -> Option<[u32; 4]> {
    let mut rest = version;
    let number = |rest: &mut &str| -> Option<u32> {
        let len = rest.bytes().take_while(u8::is_ascii_digit).count();
        if len == 0 || (len > 1 && rest.starts_with('0')) {
            return None;
        }
        let value: u32 = rest[..len].parse().ok().filter(|&v| v <= 100_000)?;
        *rest = &rest[len..];
        Some(value)
    };
    let mut parts = [0, 0, 0, u32::MAX];
    for (i, part) in parts.iter_mut().take(3).enumerate() {
        *part = number(&mut rest)?;
        if i < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    // The 3.0 hosts report "3.0.0-beta.N". The number stops at 100000, below one stage.
    const STAGE: u32 = 1_000_000;
    if let Some((stage, after)) = ["-alpha.", "-beta.", "-rc."]
        .iter()
        .enumerate()
        .find_map(|(stage, tag)| rest.strip_prefix(tag).map(|after| (stage as u32 + 1, after)))
    {
        rest = after;
        parts[3] = stage * STAGE + number(&mut rest).filter(|&pre| pre > 0)?;
    }
    if let Some(build) = rest.strip_prefix('+') {
        let len = build
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == b'.' || *c == b'-')
            .count();
        if len == 0 {
            return None;
        }
        rest = &build[len..];
    }
    rest.is_empty().then_some(parts)
}

const HOST_DISPLAY_MODES_VERSION: [u32; 4] = [2, 2, 1, u32::MAX];

impl Evidence {
    pub(crate) fn parse(root: &Element) -> Self {
        let mut e = Evidence {
            permission: -1,
            hardware_metrics_version: -1,
            network_path_version: -1,
            cursor_shape_version: -1,
            native_api_version: -1,
            ..Default::default()
        };
        let (mut encoder, mut initial_encoder) = (false, false);
        for field in root.elements() {
            let text = field.text();
            match field.name.as_str() {
                // The MJOLNIR codename was used by GFE but never by any third-party server.
                "state" => e.game_stream = text.contains("MJOLNIR"),
                "appversion" => e.sunshine_version = text.trim().ends_with(".-1"),
                "PairStatus" => e.paired = text.trim() == "1",
                "RustHostVersion" => e.rust_host_version = Some(text.trim().to_owned()),
                "Permission" => e.permission = unsigned(&text, 0xFFFF_FFFF) as i64,
                "ServerCommand" => e.server_commands += 1,
                "VirtualDisplayCapable" => e.virtual_display_capable = flag(&text),
                "VirtualDisplayDriverReady" => e.virtual_display_driver_ready = flag(&text),
                "VirtualDisplayHDRCapable" => e.virtual_display_hdr_capable = flag(&text),
                "FrameLimiterSupported" => e.frame_limiter = Some(flag(&text) == Some(true)),
                "PyroWaveHostLinkMbps" => e.pyrowave_fields = true,
                "PyroWaveBandwidthProbeBytes" => {
                    e.pyrowave_fields = true;
                    e.pyrowave_probe_bytes =
                        unsigned(&text, PYROWAVE_BANDWIDTH_PROBE_BYTES as u64) as u32;
                }
                "ServerCodecModeSupport" => {
                    e.pyrowave_codecs = unsigned(&text, 0xFFFF_FFFF) as u32 & PYROWAVE_CODEC_BITS
                }
                "RustHostEncoderBitrateKbps" => encoder = true,
                "RustHostInitialEncoderBitrateKbps" => initial_encoder = true,
                "HardwareMetricsVersion" => {
                    e.hardware_metrics_version = i32::from(text.trim() == "1")
                }
                "NetworkPathVersion" => e.network_path_version = i32::from(text.trim() == "1"),
                "ControllerPreparation" => e.controller_preparation = text.trim() == "1",
                "CursorShapeVersion" => e.cursor_shape_version = unsigned(&text, 0xFFFF) as i32,
                "NativeApiVersion" => e.native_api_version = unsigned(&text, 0xFFFF) as i32,
                "currentgameuuid" => e.current_game_uuid = true,
                _ => {}
            }
        }
        e.encoder_bitrate = encoder && initial_encoder;
        if e.rust_host_version.as_deref() == Some("") {
            e.rust_host_version = None;
        }
        e
    }

    fn family(&self) -> Family {
        if self.game_stream {
            Family::GameStream
        } else if self.rust_host_version.is_some() {
            Family::Rubylight
        } else if self.pyrowave_fields || self.frame_limiter.is_some() {
            Family::Vibepollo
        } else if self.permission != -1
            || self.server_commands > 0
            || self.virtual_display_capable.is_some()
        {
            Family::Apollo
        } else if self.sunshine_version {
            Family::Sunshine
        } else {
            Family::Unknown
        }
    }

    fn has_permission(&self, mask: u32) -> bool {
        self.permission == -1 || (self.permission as u32 & mask) != 0
    }

    fn can_view(&self) -> bool {
        self.has_permission(PERMISSION_VIEW | PERMISSION_LAUNCH)
    }

    /// Rubylight, or the extension fields that Vibepollo added and Rubylight also sends.
    fn vibepollo_extensions(&self) -> bool {
        self.rust_host_version.is_some() || self.frame_limiter.is_some()
    }

    /// No evidence: an unrecognized host rules nothing out; Apollo and Vibepollo hide some
    /// fields from unpaired clients.
    fn absent(&self, hidden_until_paired: bool) -> Availability {
        let family = self.family();
        if family == Family::Unknown
            || (hidden_until_paired
                && !self.paired
                && matches!(family, Family::Apollo | Family::Vibepollo))
        {
            Availability::Unknown
        } else {
            Availability::NEVER
        }
    }

    fn rubylight_version(&self) -> Option<[u32; 4]> {
        self.rust_host_version
            .as_deref()
            .and_then(parse_rubylight_version)
    }

    fn virtual_display(&self) -> Availability {
        match self.virtual_display_capable {
            None => self.absent(true),
            Some(false) if self.rust_host_version.is_some() => {
                Availability::Unsupported(Some(Reason::VirtualDisplayOff))
            }
            Some(false) => Availability::NEVER,
            Some(true) if self.virtual_display_driver_ready == Some(false) => {
                Availability::Unsupported(Some(Reason::VirtualDisplayDriver))
            }
            Some(true) => Availability::Supported,
        }
    }

    fn availability(&self, feature: Feature) -> Availability {
        use Availability::{Supported, Unknown, Unsupported};
        let rubylight = self.rust_host_version.is_some();
        let update = Unsupported(Some(Reason::UpdateHost));
        let pair = Unsupported(Some(Reason::Pair));
        match feature {
            Feature::Permissions => {
                if self.permission != -1 {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::OtpPairing => {
                if self.permission != -1 || rubylight {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::Clipboard => {
                if self.permission == -1 {
                    self.absent(false)
                } else if !self.paired {
                    pair
                } else if self.can_view()
                    && self.permission as u32
                        & (PERMISSION_CLIPBOARD_READ | PERMISSION_CLIPBOARD_SET)
                        != 0
                {
                    Supported
                } else {
                    Unsupported(Some(Reason::ClipboardPermission))
                }
            }
            Feature::ServerCommands => {
                if self.permission == -1 {
                    if self.server_commands > 0 {
                        Supported
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if self.permission as u32 & PERMISSION_SERVER_COMMAND == 0 {
                    Unsupported(Some(Reason::CommandPermission))
                } else if self.server_commands > 0 {
                    Supported
                } else {
                    Unsupported(Some(Reason::NoCommands))
                }
            }
            Feature::AppUuids => {
                if self.current_game_uuid {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            // Rubylight and Vibepollo look an appuuid up before the ID.
            Feature::LaunchByUuid => {
                if self.vibepollo_extensions() {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::VirtualDisplay => self.virtual_display(),
            Feature::VirtualDisplayHdr => {
                let display = self.virtual_display();
                if !display.is_supported() {
                    display
                } else {
                    match self.virtual_display_hdr_capable {
                        None => self.absent(false),
                        Some(true) => Supported,
                        Some(false) => Availability::NEVER,
                    }
                }
            }
            Feature::HostDisplayModes => {
                if !rubylight {
                    self.absent(false)
                } else if !self
                    .rubylight_version()
                    .is_some_and(|v| v >= HOST_DISPLAY_MODES_VERSION)
                {
                    update
                } else {
                    self.virtual_display()
                }
            }
            // Rubylight, Apollo and Vibepollo read "59.94"; Sunshine truncates it to 59.
            Feature::FractionalRefresh => {
                if self.vibepollo_extensions() || self.permission != -1 {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::HostTiming => {
                if rubylight {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::HostVrr | Feature::RemoteSessionTiles | Feature::HostConfirmsReplacement => {
                if self.vibepollo_extensions() {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::RuntimeBitrate => {
                let missing = if rubylight {
                    !self.encoder_bitrate
                } else {
                    self.frame_limiter.is_none()
                };
                if missing {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if self.can_view() {
                    Supported
                } else {
                    Unsupported(Some(Reason::ViewPermission))
                }
            }
            Feature::PyroWave => {
                if !self.pyrowave_fields && !rubylight {
                    self.absent(true)
                } else if self.pyrowave_codecs != 0 {
                    Supported
                } else {
                    Availability::NEVER
                }
            }
            Feature::PyroWaveRecords => {
                let pyrowave = self.availability(Feature::PyroWave);
                if !pyrowave.is_supported() {
                    pyrowave
                } else if !rubylight || self.rubylight_version().is_some_and(|v| v[0] >= 2) {
                    // Rubylight frames records since 2.0.0-rc.1; Vibepollo since it offered PyroWave.
                    Supported
                } else {
                    Availability::NEVER
                }
            }
            Feature::PyroWaveBandwidthProbe => {
                if !self.pyrowave_fields {
                    if rubylight {
                        update
                    } else {
                        self.absent(true)
                    }
                } else if !self.paired {
                    pair
                } else if self.pyrowave_probe_bytes == PYROWAVE_BANDWIDTH_PROBE_BYTES {
                    Supported
                } else {
                    Availability::NEVER
                }
            }
            Feature::HardwareMetrics => {
                if self.hardware_metrics_version == -1 {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if !self.can_view() {
                    Unsupported(Some(Reason::ViewPermission))
                } else if self.hardware_metrics_version == 1 {
                    Supported
                } else {
                    Unsupported(Some(Reason::HardwareMetricsOff))
                }
            }
            Feature::NetworkPath => {
                if self.network_path_version == -1 {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired {
                    pair
                } else if self.network_path_version == 1 {
                    Supported
                } else {
                    Unsupported(Some(Reason::ViewPermission))
                }
            }
            Feature::FrameLimiterStatus => {
                if self.frame_limiter == Some(true) {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            Feature::ControllerPreparation => {
                if self.controller_preparation {
                    Supported
                } else if rubylight {
                    update
                } else {
                    self.absent(false)
                }
            }
            Feature::WebUi => {
                if self.game_stream {
                    Availability::NEVER
                } else if self.family() != Family::Unknown {
                    Supported
                } else {
                    Unknown
                }
            }
            Feature::CursorShape => {
                if self.cursor_shape_version >= 1 {
                    Supported
                } else {
                    self.absent(false)
                }
            }
            // The host reports 0 over plain HTTP and to an unpaired certificate.
            Feature::NativeApi => {
                if self.native_api_version == -1 {
                    if rubylight {
                        update
                    } else {
                        self.absent(false)
                    }
                } else if !self.paired || self.native_api_version < 1 {
                    pair
                } else {
                    Supported
                }
            }
        }
    }
}

impl HostCapabilities {
    /// A host never reached from this device: nothing is known, so nothing is ruled out.
    pub const UNKNOWN: HostCapabilities = HostCapabilities { evidence: None };

    pub fn from_evidence(evidence: Evidence) -> Self {
        Self {
            evidence: Some(evidence),
        }
    }

    pub fn evidence(&self) -> Option<&Evidence> {
        self.evidence.as_ref()
    }

    pub fn is_known(&self) -> bool {
        self.evidence.is_some()
    }

    pub fn family(&self) -> Family {
        self.evidence
            .as_ref()
            .map_or(Family::Unknown, Evidence::family)
    }

    /// The Rubylight version the host reports, or None for other hosts.
    pub fn host_version(&self) -> Option<&str> {
        self.evidence.as_ref()?.rust_host_version.as_deref()
    }

    pub fn availability(&self, feature: Feature) -> Availability {
        self.evidence
            .as_ref()
            .map_or(Availability::Unknown, |e| e.availability(feature))
    }

    pub fn has(&self, feature: Feature) -> bool {
        self.availability(feature).is_supported()
    }

    /// Supported or not known yet: for behavior that does no harm on a host without it.
    pub fn may_have(&self, feature: Feature) -> bool {
        !matches!(self.availability(feature), Availability::Unsupported(_))
    }

    pub fn supported(&self) -> Vec<Feature> {
        Feature::ALL.into_iter().filter(|&f| self.has(f)).collect()
    }

    /// The codec support to negotiate with: PyroWave bits count only from a host that shows
    /// PyroWave.
    pub fn trusted_codec_mode_support(&self, server_codec_mode_support: u32) -> u32 {
        if self.has(Feature::PyroWave) {
            server_codec_mode_support
        } else {
            server_codec_mode_support & !PYROWAVE_CODEC_BITS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubylight_versions() {
        assert_eq!(
            parse_rubylight_version("2.2.21"),
            Some([2, 2, 21, u32::MAX])
        );
        assert_eq!(parse_rubylight_version("2.0.0-rc.19"), Some([2, 0, 0, 3_000_019]));
        assert_eq!(parse_rubylight_version("3.0.0-beta.1"), Some([3, 0, 0, 2_000_001]));
        assert_eq!(parse_rubylight_version("3.0.0-alpha.2+abc.1"), Some([3, 0, 0, 1_000_002]));
        // alpha < beta < rc < release, whatever the numbers.
        let ascending = ["3.0.0-alpha.1", "3.0.0-alpha.99", "3.0.0-beta.1", "3.0.0-beta.12", "3.0.0-rc.1", "3.0.0-rc.30", "3.0.0"];
        for pair in ascending.windows(2) {
            assert!(parse_rubylight_version(pair[0]).unwrap() < parse_rubylight_version(pair[1]).unwrap(), "{pair:?}");
        }
        assert_eq!(
            parse_rubylight_version("2.2.1+abc.1"),
            Some([2, 2, 1, u32::MAX])
        );
        for bad in ["", "2.2", "02.1.1", "2.2.1-rc.0", "2.2.1+", "2.2.1x", "3.0.0-beta", "3.0.0-beta.0", "3.0.0-gamma.1"] {
            assert_eq!(parse_rubylight_version(bad), None, "{bad}");
        }
        assert!(parse_rubylight_version("2.2.1").unwrap() >= HOST_DISPLAY_MODES_VERSION);
        assert!(parse_rubylight_version("2.2.1-rc.3").unwrap() < HOST_DISPLAY_MODES_VERSION);
    }

    #[test]
    fn unknown_hosts_rule_nothing_out() {
        let caps = HostCapabilities::UNKNOWN;
        assert_eq!(caps.family(), Family::Unknown);
        assert!(caps.may_have(Feature::RemoteSessionTiles));
        assert!(!caps.has(Feature::PyroWave));
        assert_eq!(caps.trusted_codec_mode_support(0x0180_0101), 0x0000_0101);
    }
}
